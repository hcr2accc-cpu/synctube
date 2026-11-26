//! The client core: connection, clock sync, stream resolving and the sync
//! loop that keeps the local player on the shared timeline. Runs on its own
//! thread with a single-threaded tokio runtime; the UI talks to it through
//! a command channel and reads a shared `View` snapshot.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shared::sync::{
    decide_drift, decide_drift_low, expected_position_clamped, to_server_ms, DriftAction,
};
use shared::{ClientMsg, Member, MemberSync, PlayStatus, RoomState, ServerMsg, TrackInfo};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::clock::{instant_at_local_ms, local_ms, ClockSync};
use crate::library::{self, Library, ListenTimer, Playlist, Song};
use crate::net::{self, NetEvent, NetHandle};
use crate::paths::Tools;
use crate::player::{FakePlayer, Player, PlayerEvent};
use crate::strings;
use crate::ytdlp::{Resolver, SearchHit, StreamInfo};
use shared::youtube::{parse_youtube_url, playlist_url, watch_url, YoutubeRef};

/// Settings for the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub fake_audio: bool,
    /// Clock rate of the fake player (1.0 = perfect), to exercise drift correction.
    pub fake_rate: f64,
    pub tools: Tools,
    pub volume: f64,
    /// Loudness normalization (same perceived volume for every song).
    pub normalize: bool,
    /// Run `yt-dlp -U` in the background on start.
    pub auto_update: bool,
    /// Local equalizer.
    pub eq: crate::eq::EqSettings,
    /// Show each song's video.
    pub video: bool,
    /// Native window inside the app that the video is drawn into.
    pub video_wid: Option<i64>,
    /// Low-latency mode: constant clock pings, a drift check 5 times a
    /// second and correction of even a few milliseconds of drift.
    pub low_latency: bool,
    /// How late this device's sound comes out (Bluetooth headphones,
    /// speakers); the player runs that much ahead to make up for it.
    pub audio_offset_ms: i64,
    /// Where the user's playlists and history are kept. `None`: nowhere
    /// (and nothing is remembered).
    pub library_dir: Option<std::path::PathBuf>,
}

/// The playlist something is saved to.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveTo {
    Existing(Uuid),
    New(String),
}

/// What is saved to one of the user's playlists.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveItem {
    Songs(Vec<Song>),
    /// Every song of a YouTube playlist (its link).
    YoutubePlaylist(String),
}

/// Commands from the UI.
#[derive(Debug, Clone)]
pub enum Command {
    Connect {
        url: String,
        room: String,
        password: String,
        name: String,
    },
    Leave,
    Play,
    Pause,
    TogglePlay,
    Next,
    Previous,
    Seek(u64),
    PlayTrack(Uuid),
    Remove(Uuid),
    Move(Uuid, usize),
    /// Shuffles the songs after the current one (all of them when nothing
    /// plays); a second one while shuffling stops it.
    Shuffle,
    /// Removes every song from the room's queue; a second one while it
    /// runs stops it.
    ClearQueue,
    /// Turns the room's radio on or off (for everyone in the room).
    SetRadio(bool),
    Add(String),
    /// Searches YouTube (videos, or playlists); results show up in
    /// [`View::search`].
    Search {
        query: String,
        playlists: bool,
    },
    ClearSearch,
    SetVolume(f64),
    SetNormalize(bool),
    SetEq(crate::eq::EqSettings),
    SetVideo(bool),
    SetLowLatency(bool),
    SetAudioOffset(i64),
    SetTools(Tools),
    /// Saves songs to one of the user's playlists.
    Save {
        to: SaveTo,
        item: SaveItem,
    },
    CreatePlaylist(String),
    RenamePlaylist(Uuid, String),
    DeletePlaylist(Uuid),
    /// Removes a song (by YouTube id) from a playlist.
    Unsave(Uuid, String),
    MoveSaved(Uuid, String, usize),
    /// Adds a whole playlist of the user to the room's queue.
    QueuePlaylist(Uuid),
    /// The recommendations are on screen: fetch them if they are old.
    ShowRecs,
    RefreshRecs,
    ClearHistory,
    Shutdown,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ConnState {
    #[default]
    Idle,
    Connecting {
        waking: bool,
    },
    Connected,
    Reconnecting {
        retry_in_s: u64,
    },
    Rejected(String),
    Failed(String),
}

/// What the local player is doing.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Resolving,
    Buffering,
    /// Loaded and positioned, waiting for the scheduled start / other clients.
    Waiting,
    Playing,
    Paused,
    Error(String),
}

/// A shuffle in progress: the new order of the upcoming songs.
struct Shuffle {
    order: Vec<Uuid>,
    /// Songs of `order` placed so far.
    done: usize,
    /// The song placed last; the next one goes right after it.
    prev: Option<Uuid>,
    /// Room revision and time of the last move sent.
    sent: Option<(u64, Instant)>,
}

/// A "remove everything" in progress. The server only knows single
/// removals, so the songs go one at a time.
struct Clear {
    /// Songs to remove, the playing one last (removing it earlier would
    /// start the next song for everyone).
    ids: Vec<Uuid>,
    done: usize,
    /// Room revision and time of the last removal sent.
    sent: Option<(u64, Instant)>,
}

/// A link that was submitted but is not in the queue yet. Shown right away
/// so adding feels instant.
#[derive(Debug, Clone)]
pub struct PendingAdd {
    pub playlist: bool,
    /// Known as soon as the quick lookup finishes.
    pub title: Option<String>,
    pub started: Instant,
}

/// The latest YouTube search.
#[derive(Debug, Clone, Default)]
pub struct SearchView {
    pub query: String,
    /// Playlists instead of videos.
    pub playlists: bool,
    pub loading: bool,
    pub results: Vec<SearchHit>,
    pub error: Option<String>,
}

/// The user's playlists and recommendations.
#[derive(Debug, Clone, Default)]
pub struct LibraryView {
    pub playlists: Vec<Playlist>,
    pub recs: Vec<Song>,
    pub recs_loading: bool,
    pub recs_error: Option<String>,
    pub has_history: bool,
    /// YouTube playlists being read to be saved.
    pub importing: usize,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

/// Snapshot of the engine state for the UI.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub conn: ConnState,
    pub joined: bool,
    pub room: Option<RoomState>,
    pub members: Vec<Member>,
    pub me: Option<Member>,
    pub offset_ms: i64,
    pub rtt_ms: Option<u64>,
    pub clock_synced: bool,
    pub phase: Phase,
    pub drift_ms: Option<i64>,
    pub speed: f64,
    /// The low-latency mode is on.
    pub low_latency: bool,
    pub player_pos_ms: Option<u64>,
    pub pending_adds: Vec<PendingAdd>,
    pub search: SearchView,
    /// Shared, because the UI takes a copy of the view every frame.
    pub library: Arc<LibraryView>,
    pub toasts: Vec<Toast>,
    pub volume: f64,
    pub player_error: Option<String>,
    pub ytdlp_version: Option<String>,
    pub ytdlp_updating: bool,
    pub fake_audio: bool,
    /// Bumped when the user closed the video window (video mode turned off).
    pub video_closed: u64,
    /// Songs still to be moved while a shuffle runs.
    pub shuffle_left: Option<usize>,
    /// Songs still to be removed while the queue is being cleared.
    pub clear_left: Option<usize>,
}

impl View {
    /// This client's player as the room sees it (the sync dots).
    pub fn sync(&self) -> MemberSync {
        MemberSync {
            loading: matches!(self.phase, Phase::Resolving | Phase::Buffering),
            drift_ms: match self.phase {
                Phase::Playing => self.drift_ms.map(|d| d.clamp(-10_000, 10_000) as i32),
                _ => None,
            },
        }
    }

    pub fn server_now_ms(&self) -> u64 {
        to_server_ms(local_ms(), self.offset_ms)
    }

    /// Timeline position of the current track (what everyone should hear now).
    pub fn position_ms(&self) -> Option<u64> {
        let room = self.room.as_ref()?;
        let track = room.current_track()?;
        Some(expected_position_clamped(
            &room.playback,
            self.server_now_ms(),
            track.duration_ms,
        ))
    }

    pub fn is_playing(&self) -> bool {
        self.room
            .as_ref()
            .is_some_and(|r| r.playback.status == PlayStatus::Playing)
    }
}

/// Handle owned by the UI.
pub struct EngineHandle {
    cmd: mpsc::UnboundedSender<Command>,
    pub view: Arc<Mutex<View>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        let _ = self.cmd.send(c);
    }

    pub fn view(&self) -> View {
        self.view.lock().unwrap().clone()
    }

    /// Stops the engine (leaves the room, quits mpv) and waits briefly.
    pub fn shutdown(mut self) {
        let _ = self.cmd.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Starts the engine thread. `notify` is called whenever the view changes.
pub fn start(cfg: EngineConfig, notify: Arc<dyn Fn() + Send + Sync>) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let view = Arc::new(Mutex::new(View {
        volume: cfg.volume,
        speed: 1.0,
        fake_audio: cfg.fake_audio,
        ..View::default()
    }));
    let shared_view = view.clone();
    let thread = std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let engine = Engine::new(cfg, shared_view, notify);
                engine.run(cmd_rx).await;
            });
        })
        .expect("engine thread");
    EngineHandle {
        cmd: cmd_tx,
        view,
        thread: Some(thread),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CurPhase {
    Resolving,
    Loading,
    Seeking,
    /// Loaded, but held (paused) until its loudness is known, so the song
    /// does not start at the wrong level and jump a moment later.
    AwaitGain,
    Active,
    Failed,
}

/// The track the local player is working on.
struct Cur {
    track_id: Uuid,
    youtube_id: String,
    duration_ms: Option<u64>,
    phase: CurPhase,
    entry: u64,
    paused: bool,
    speed: f64,
    reresolved: bool,
    /// Loudness still being measured: wait for it at most until then.
    gain_deadline: Option<Instant>,
    /// The song's video stream (for video mode).
    video_url: Option<String>,
    /// The video track is open in the player.
    video_loaded: bool,
    /// Playing from the disk cache.
    local: bool,
}

enum TaskResult {
    Stream {
        track_id: Uuid,
        result: Result<StreamInfo, String>,
    },
    Meta {
        job: u64,
        result: Result<Vec<TrackInfo>, String>,
        /// Came from the quick title lookup (duration still unknown).
        quick: bool,
    },
    Duration {
        youtube_id: String,
        result: Result<StreamInfo, String>,
    },
    YtdlpVersion(Option<String>),
    Search {
        seq: u64,
        result: Result<Vec<SearchHit>, String>,
    },
    /// The songs of a YouTube playlist that is being saved.
    Import {
        playlist: Uuid,
        result: Result<Vec<TrackInfo>, String>,
    },
    Mix {
        run: u64,
        weight: f64,
        result: Result<Vec<SearchHit>, String>,
    },
    /// Songs found for the server's `RadioRequest` (empty when none).
    Radio {
        request_id: u64,
        tracks: Vec<TrackInfo>,
    },
    Loudness {
        youtube_id: String,
        result: Result<crate::loudness::Loudness, String>,
    },
    Prepared {
        youtube_id: String,
        result: Result<(), String>,
    },
}

const PING_BURST: u8 = 8;
const PING_SPACING: Duration = Duration::from_millis(100);
const RESYNC_EVERY: Duration = Duration::from_secs(30);
const DRIFT_EVERY: Duration = Duration::from_secs(1);
/// Low-latency mode: one clock ping this often, all the time.
const LOW_PING_EVERY: Duration = Duration::from_millis(250);
/// Low-latency mode: drift check this often.
const LOW_DRIFT_EVERY: Duration = Duration::from_millis(200);
/// At most this often while only the drift number changes.
const STATUS_MIN_GAP: Duration = Duration::from_millis(400);
/// Wake up this much before a scheduled unpause and spin the rest (Windows
/// timers are coarse).
const SPIN_MS: u64 = 16;
/// Max wait for the timeline to reach a pre-seeked position.
const MAX_WAIT_MS: i64 = 6000;
const MAX_TOASTS: usize = 20;
/// Drop a pending add if the server has not answered the `Add` by then.
const ADD_ANSWER_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest a freshly loaded song waits for its loudness measurement.
const GAIN_WAIT: Duration = Duration::from_millis(2500);
/// How many upcoming songs are measured ahead, one at a time.
const MEASURE_AHEAD: usize = 5;
/// The next song starts downloading to disk this long after the current one
/// started playing (its own buffer fills first).
/// Gap between the moves of a shuffle (the server allows 20 messages/s).
const SHUFFLE_GAP: Duration = Duration::from_millis(70);
/// A move the server has not answered yet is given up on after this long.
const SHUFFLE_WAIT: Duration = Duration::from_secs(1);
const PREPARE_DELAY: Duration = Duration::from_secs(3);

/// One submitted link, from `Add` until `AddResolved` is sent.
struct AddJob {
    id: u64,
    /// Canonical URL; the server echoes it in `ResolveRequest`.
    url: String,
    playlist: bool,
    request_id: Option<u64>,
    result: Option<(Result<Vec<TrackInfo>, String>, bool)>,
    started: Instant,
    /// A batch of one of the user's playlists (see `bulk`).
    bulk: bool,
}

/// Recommendations being put together, one mix at a time.
struct RecRun {
    seeds: Vec<(Song, f64)>,
    /// Seeds whose mix has been asked for.
    asked: usize,
    mixes: Vec<(f64, Vec<Song>)>,
}

struct Engine {
    cfg: EngineConfig,
    view: View,
    shared: Arc<Mutex<View>>,
    notify: Arc<dyn Fn() + Send + Sync>,

    net: Option<NetHandle>,
    net_tx: mpsc::UnboundedSender<NetEvent>,
    net_rx: mpsc::UnboundedReceiver<NetEvent>,

    player: Option<Player>,
    player_tx: mpsc::UnboundedSender<PlayerEvent>,
    player_rx: mpsc::UnboundedReceiver<PlayerEvent>,
    player_restarts: u32,

    resolver: Resolver,
    task_tx: mpsc::UnboundedSender<TaskResult>,
    task_rx: mpsc::UnboundedReceiver<TaskResult>,

    clock: ClockSync,
    room: Option<RoomState>,
    cur: Option<Cur>,
    applied_version: Option<u64>,
    ready_sent: Option<u64>,
    ended_sent: Option<u64>,
    unpause_at: Option<Instant>,
    align_attempts: u32,
    /// Estimated time from `loadfile` to playable (adaptive).
    load_comp_ms: u64,
    /// Estimated time for a seek inside a loaded stream (adaptive).
    seek_comp_ms: u64,
    /// When the current track was picked (for timing logs).
    track_t0: Option<Instant>,
    op_started: Option<Instant>,
    next_drift: Option<Instant>,
    next_ping: Option<Instant>,
    pings_left: u8,
    adds: Vec<AddJob>,
    next_add_id: u64,
    /// Normalization gain (dB) per YouTube id, measured once per session.
    gains: HashMap<String, f64>,
    measuring: HashSet<String>,
    /// Bumped per search, so a slow old search never replaces a newer one.
    search_seq: u64,
    /// Not retried (until the app restarts), so a bad stream is scanned once.
    measure_failed: HashSet<String>,
    /// Scanned again once with a fresh URL (a failed scan often means the
    /// cached URL is bad, which would also stall the song's start).
    measure_retried: HashSet<String>,
    /// The upcoming song being measured in the background (one at a time).
    measuring_ahead: Option<String>,
    /// Measured loudness, kept on disk so a song is scanned only once.
    loudness_store: crate::loudness::Store,
    /// Upcoming songs' audio on disk, so a skip starts at once.
    cache: Option<crate::cache::AudioCache>,
    /// When to start preparing the next song.
    prepare_at: Option<Instant>,
    /// The song being downloaded to the cache (one at a time).
    preparing: Option<String>,
    /// Like `load_comp_ms`, for songs loaded from the cache.
    load_comp_local_ms: u64,
    /// The server relays member status (older servers reject the message).
    status_ok: bool,
    /// Last status reported to the room, and when.
    status_sent: Option<(MemberSync, Instant)>,
    shuffle: Option<Shuffle>,
    clear: Option<Clear>,

    library: Library,
    /// The library changed since the view was published.
    library_dirty: bool,
    listen: ListenTimer,
    rec_run: Option<RecRun>,
    /// Bumped per run, so answers to an old one are ignored.
    rec_seq: u64,
    recs_error: Option<String>,
    importing: usize,
    /// Batches of the user's playlists on their way to the queue. The next
    /// one goes when the one before it was sent.
    bulk: VecDeque<Vec<TrackInfo>>,
}

async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}

/// Gain for a song not measured yet: the median of the measured ones, so
/// the correction once it is known is small.
fn typical_gain(gains: &HashMap<String, f64>) -> f64 {
    if gains.len() < 3 {
        return 0.0;
    }
    let mut g: Vec<f64> = gains.values().copied().collect();
    g.sort_by(f64::total_cmp);
    g[g.len() / 2]
}

/// Radio adds at most this many songs per request (the server caps it too).
const RADIO_BATCH: usize = 5;

/// The first songs of a mix that aren't queued yet (and no repeats).
fn radio_pick(hits: Vec<SearchHit>, queued: &HashSet<String>) -> Vec<TrackInfo> {
    let mut seen = queued.clone();
    hits.into_iter()
        .filter(|h| !h.playlist && seen.insert(h.youtube_id.clone()))
        .take(RADIO_BATCH)
        .map(|h| TrackInfo {
            youtube_id: h.youtube_id,
            title: h.title,
            duration_ms: h.duration_ms,
        })
        .collect()
}

fn ewma(old: u64, sample: u64) -> u64 {
    (old * 3 + sample) / 4
}

impl Engine {
    fn new(
        cfg: EngineConfig,
        shared: Arc<Mutex<View>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (net_tx, net_rx) = mpsc::unbounded_channel();
        let (player_tx, player_rx) = mpsc::unbounded_channel();
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let view = shared.lock().unwrap().clone();
        let mut clock = ClockSync::default();
        clock.set_low_latency(cfg.low_latency);
        Self {
            resolver: Resolver::new(cfg.tools.yt_dlp.clone(), cfg.fake_audio),
            cfg,
            view,
            shared,
            notify,
            net: None,
            net_tx,
            net_rx,
            player: None,
            player_tx,
            player_rx,
            player_restarts: 0,
            task_tx,
            task_rx,
            clock,
            room: None,
            cur: None,
            applied_version: None,
            ready_sent: None,
            ended_sent: None,
            unpause_at: None,
            align_attempts: 0,
            load_comp_ms: 1500,
            seek_comp_ms: 300,
            track_t0: None,
            op_started: None,
            next_drift: None,
            next_ping: None,
            pings_left: 0,
            adds: Vec::new(),
            next_add_id: 0,
            gains: HashMap::new(),
            measuring: HashSet::new(),
            search_seq: 0,
            measure_failed: HashSet::new(),
            measure_retried: HashSet::new(),
            measuring_ahead: None,
            loudness_store: crate::loudness::Store::default(),
            cache: None,
            prepare_at: None,
            preparing: None,
            load_comp_local_ms: 250,
            status_ok: false,
            status_sent: None,
            shuffle: None,
            clear: None,
            library: Library::default(),
            library_dirty: true,
            listen: ListenTimer::default(),
            rec_run: None,
            rec_seq: 0,
            recs_error: None,
            importing: 0,
            bulk: VecDeque::new(),
        }
    }

    async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<Command>) {
        self.start_ytdlp_maintenance();
        self.library = Library::load(self.cfg.library_dir.as_deref());
        if !self.cfg.fake_audio {
            self.loudness_store =
                crate::loudness::Store::load(crate::paths::app_dir().join("loudness.json"));
            for (id, l) in self.loudness_store.iter() {
                self.gains.insert(id.clone(), l.gain_db());
            }
            self.cache = Some(crate::cache::AudioCache::new(
                crate::paths::app_dir().join("cache"),
            ));
        }
        loop {
            let spin_target = self.unpause_at;
            let wake_unpause = spin_target.map(|t| t - Duration::from_millis(SPIN_MS));
            let gain_hold = self.gain_hold_deadline();
            let shuffle_at = self.shuffle_at();
            let clear_at = self.clear_at();
            tokio::select! {
                c = cmd_rx.recv() => {
                    match c {
                        Some(Command::Shutdown) | None => {
                            self.shutdown().await;
                            return;
                        }
                        Some(c) => self.on_command(c).await,
                    }
                }
                Some(ev) = self.net_rx.recv() => self.on_net(ev).await,
                Some(ev) = self.player_rx.recv() => self.on_player(ev).await,
                Some(r) = self.task_rx.recv() => self.on_task(r).await,
                _ = sleep_opt(wake_unpause) => {
                    if let Some(t) = spin_target {
                        while Instant::now() < t {
                            std::hint::spin_loop();
                        }
                        self.unpause_now().await;
                    }
                }
                _ = sleep_opt(self.next_drift) => self.drift_check().await,
                _ = sleep_opt(self.next_ping) => self.ping_tick(),
                _ = sleep_opt(gain_hold) => self.release_gain_hold().await,
                _ = sleep_opt(self.prepare_at) => self.prepare_next(),
                _ = sleep_opt(shuffle_at) => self.shuffle_step(),
                _ = sleep_opt(clear_at) => self.clear_step(),
            }
            self.publish();
        }
    }

    // ----- view ---------------------------------------------------------------

    fn publish(&mut self) {
        self.view.room = self.room.clone();
        self.view.offset_ms = self.clock.offset_ms();
        self.view.rtt_ms = self.clock.rtt_ms();
        self.view.clock_synced = self.clock.is_synced();
        self.view.phase = self.phase();
        self.view.speed = self.cur.as_ref().map_or(1.0, |c| c.speed);
        self.view.low_latency = self.cfg.low_latency;
        self.view.shuffle_left = self.shuffle.as_ref().map(|s| s.order.len() - s.done);
        self.view.clear_left = self.clear.as_ref().map(|c| c.ids.len() - c.done);
        self.report_status();
        self.count_listening();
        self.adds
            .retain(|j| j.request_id.is_some() || j.started.elapsed() < ADD_ANSWER_TIMEOUT);
        if !self.bulk.is_empty() && !self.adds.iter().any(|j| j.bulk) {
            // The server never answered, so the rest will not get through either.
            self.bulk.clear();
        }
        self.publish_library();
        self.view.pending_adds = self
            .adds
            .iter()
            .map(|j| PendingAdd {
                playlist: j.playlist,
                title: match &j.result {
                    Some((Ok(t), _)) if t.len() == 1 => Some(t[0].title.clone()),
                    _ => None,
                },
                started: j.started,
            })
            .collect();
        if self.view.toasts.len() > MAX_TOASTS {
            let n = self.view.toasts.len() - MAX_TOASTS;
            self.view.toasts.drain(..n);
        }
        *self.shared.lock().unwrap() = self.view.clone();
        (self.notify)();
    }

    fn phase(&self) -> Phase {
        let Some(room) = &self.room else {
            return Phase::Idle;
        };
        let Some(cur) = &self.cur else {
            return Phase::Idle;
        };
        match cur.phase {
            CurPhase::Resolving => Phase::Resolving,
            CurPhase::Loading | CurPhase::Seeking | CurPhase::AwaitGain => Phase::Buffering,
            CurPhase::Failed => Phase::Error(strings::RESOLVE_FAILED.into()),
            CurPhase::Active => match room.playback.status {
                PlayStatus::Playing if self.unpause_at.is_some() || cur.paused => Phase::Waiting,
                PlayStatus::Playing => Phase::Playing,
                PlayStatus::Paused if room.waiting_for_ready => Phase::Waiting,
                PlayStatus::Paused => Phase::Paused,
                PlayStatus::Stopped => Phase::Idle,
            },
        }
    }

    /// Tells the room how this player is doing, when that changes.
    fn report_status(&mut self) {
        if !self.status_ok || !self.view.joined {
            return;
        }
        let mut now = self.view.sync();
        // Drift inside the tolerance is "in sync"; beyond it, coarse steps
        // are enough for a dot on a screen.
        let low = self.cfg.low_latency;
        now.drift_ms = now.drift_ms.map(|d| {
            if low {
                // Exact, so the room sees the difference the mode makes.
                if (d as i64).abs() <= shared::sync::LOW_DEADBAND_MS {
                    0
                } else {
                    d
                }
            } else if (d as i64).abs() <= shared::sync::DRIFT_IGNORE_MS {
                0
            } else {
                d / 20 * 20
            }
        });
        if let Some((last, at)) = self.status_sent {
            let same_kind =
                last.loading == now.loading && last.drift_ms.is_some() == now.drift_ms.is_some();
            if last == now || (same_kind && at.elapsed() < STATUS_MIN_GAP) {
                return;
            }
        }
        self.status_sent = Some((now, Instant::now()));
        self.send(ClientMsg::Status {
            loading: now.loading,
            drift_ms: now.drift_ms,
        });
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        tracing::info!(error, "toast: {text}");
        self.view.toasts.push(Toast {
            text,
            at: Instant::now(),
            error,
        });
    }

    fn send(&self, msg: ClientMsg) {
        if let Some(n) = &self.net {
            n.send(msg);
        }
    }

    fn server_now(&self) -> u64 {
        self.clock.server_now_ms()
    }

    // ----- commands -------------------------------------------------------------

    async fn on_command(&mut self, c: Command) {
        let version = self.room.as_ref().map(|r| r.playback.version);
        match c {
            Command::Connect {
                url,
                room,
                password,
                name,
            } => self.connect(url, room, password, name).await,
            Command::Leave => self.leave().await,
            Command::Play => self.send(ClientMsg::Play),
            Command::Pause => self.send(ClientMsg::Pause),
            Command::TogglePlay => {
                if self.view.is_playing() || self.room.as_ref().is_some_and(|r| r.waiting_for_ready)
                {
                    self.send(ClientMsg::Pause)
                } else {
                    self.send(ClientMsg::Play)
                }
            }
            Command::Next => self.send(ClientMsg::Next { version }),
            Command::Previous => self.send(ClientMsg::Previous { version }),
            Command::Seek(position_ms) => self.send(ClientMsg::Seek { position_ms }),
            Command::PlayTrack(track_id) => self.send(ClientMsg::PlayTrack { track_id }),
            Command::Remove(track_id) => self.send(ClientMsg::Remove { track_id }),
            Command::Move(track_id, new_index) => self.send(ClientMsg::Move {
                track_id,
                new_index,
            }),
            Command::Shuffle => self.start_shuffle(),
            Command::ClearQueue => self.start_clear(),
            Command::SetRadio(on) => self.send(ClientMsg::SetRadio { on }),
            Command::Add(url) => match parse_youtube_url(url.trim()) {
                None => self.toast(strings::NOT_YOUTUBE, true),
                Some(r) => {
                    let url = self.start_add_job(r, None);
                    self.send(ClientMsg::Add { url });
                }
            },
            Command::Search { query, playlists } => {
                let query = query.trim().to_string();
                if query.is_empty() {
                    return;
                }
                self.search_seq += 1;
                let seq = self.search_seq;
                self.view.search = SearchView {
                    query: query.clone(),
                    playlists,
                    loading: true,
                    ..SearchView::default()
                };
                let resolver = self.resolver.clone();
                let tx = self.task_tx.clone();
                tokio::spawn(async move {
                    let result = resolver.search(&query, playlists).await;
                    let _ = tx.send(TaskResult::Search { seq, result });
                });
            }
            Command::ClearSearch => {
                self.search_seq += 1;
                self.view.search = SearchView::default();
            }
            Command::SetVolume(v) => {
                self.view.volume = v.clamp(0.0, 100.0);
                if let Some(p) = &self.player {
                    p.set_volume(self.view.volume).await;
                }
            }
            Command::SetNormalize(on) => {
                self.cfg.normalize = on;
                let id = self.cur.as_ref().map(|c| c.youtube_id.clone());
                if let Some(id) = &id {
                    self.measure_loudness(id, None);
                }
                self.apply_gain().await;
            }
            Command::SetEq(eq) => self.set_eq(eq.clamped()).await,
            Command::SetVideo(on) => self.set_video(on).await,
            Command::SetLowLatency(on) => {
                self.cfg.low_latency = on;
                self.clock.set_low_latency(on);
                if self.net.is_some() {
                    self.pings_left = 0;
                    self.next_ping = Some(Instant::now());
                }
                if self.next_drift.is_some() {
                    self.schedule_drift();
                }
                if !on {
                    self.set_speed(1.0).await;
                }
            }
            Command::SetAudioOffset(ms) => {
                self.cfg.audio_offset_ms = ms.clamp(-500, 500);
                if self.next_drift.is_some() {
                    self.schedule_drift();
                }
            }
            Command::SetTools(tools) => {
                let had_ytdlp = self.cfg.tools.yt_dlp.is_some();
                self.resolver.set_path(tools.yt_dlp.clone());
                self.cfg.tools = tools;
                if !had_ytdlp {
                    self.start_ytdlp_maintenance();
                }
                if self.player.is_none() && self.view.joined {
                    self.ensure_player().await;
                    self.reconcile().await;
                }
            }
            Command::Save { to, item } => self.save(to, item),
            Command::CreatePlaylist(name) => {
                if self.library.create(&name).is_some() {
                    self.playlists_changed();
                }
            }
            Command::RenamePlaylist(id, name) => {
                self.library.rename(id, &name);
                self.playlists_changed();
            }
            Command::DeletePlaylist(id) => {
                self.library.delete(id);
                self.playlists_changed();
            }
            Command::Unsave(id, youtube_id) => {
                self.library.remove(id, &youtube_id);
                self.playlists_changed();
            }
            Command::MoveSaved(id, youtube_id, to) => {
                self.library.move_song(id, &youtube_id, to);
                self.playlists_changed();
            }
            Command::QueuePlaylist(id) => {
                if let Some(p) = self.library.get(id) {
                    self.bulk.extend(library::batches(&p.tracks));
                    self.send_batch();
                }
            }
            Command::ShowRecs => {
                if self.library.recs_stale(library::unix_now()) {
                    self.start_recs();
                }
            }
            Command::RefreshRecs => self.start_recs(),
            Command::ClearHistory => {
                self.rec_seq += 1;
                self.rec_run = None;
                self.recs_error = None;
                self.library.clear_history();
                self.history_changed();
            }
            Command::Shutdown => {}
        }
    }

    async fn connect(&mut self, url: String, room: String, password: String, name: String) {
        if let Some(n) = self.net.take() {
            n.close().await;
        }
        self.reset_session().await;
        let join = ClientMsg::Join {
            room,
            password: (!password.is_empty()).then_some(password),
            name,
            radio: true,
        };
        self.view.conn = ConnState::Connecting { waking: false };
        self.net = Some(net::spawn(url, join, self.net_tx.clone()));
        self.ensure_player().await;
    }

    async fn leave(&mut self) {
        if let Some(n) = self.net.take() {
            n.close().await;
        }
        self.reset_session().await;
        self.view.conn = ConnState::Idle;
    }

    async fn reset_session(&mut self) {
        self.room = None;
        self.view.joined = false;
        self.view.members.clear();
        self.view.me = None;
        self.adds.clear();
        self.bulk.clear();
        self.shuffle = None;
        self.clear = None;
        self.clock.reset();
        self.next_ping = None;
        self.unload().await;
    }

    async fn shutdown(&mut self) {
        if let Some(n) = self.net.take() {
            n.close().await;
        }
        if let Some(p) = self.player.take() {
            p.shutdown().await;
        }
        self.room = None;
        self.cur = None;
        self.publish();
    }

    // ----- network --------------------------------------------------------------

    async fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connecting { waking } => self.view.conn = ConnState::Connecting { waking },
            NetEvent::Connected => {}
            NetEvent::Disconnected { retry_in } => {
                self.view.conn = ConnState::Reconnecting {
                    retry_in_s: retry_in.as_secs(),
                };
                self.next_ping = None;
            }
            NetEvent::Rejected(reason) => {
                self.net = None;
                self.reset_session().await;
                self.view.conn = ConnState::Rejected(reason);
            }
            NetEvent::Failed(reason) => {
                tracing::warn!("connection failed: {reason}");
                self.net = None;
                self.reset_session().await;
                self.view.conn = ConnState::Failed(strings::SERVER_UNREACHABLE.into());
            }
            NetEvent::Msg(m) => self.on_server(m).await,
            NetEvent::Pong {
                client_ms,
                server_ms,
                recv_ms,
            } => self.clock.add_sample(client_ms, server_ms, recv_ms),
        }
    }

    async fn on_server(&mut self, m: ServerMsg) {
        match m {
            ServerMsg::Welcome {
                you,
                room_state,
                server_ms,
            } => {
                tracing::info!(room = %room_state.name, "joined");
                self.view.conn = ConnState::Connected;
                self.view.joined = true;
                self.status_ok = you.sync.is_some();
                self.status_sent = None;
                self.view.me = Some(you);
                self.clock.set_rough(server_ms);
                self.pings_left = 0;
                self.next_ping = Some(Instant::now());
                // Fresh state after (re)connect: re-apply everything.
                self.applied_version = None;
                self.ready_sent = None;
                self.room = Some(room_state);
                self.reconcile().await;
            }
            ServerMsg::State { room_state } => {
                if self
                    .room
                    .as_ref()
                    .is_some_and(|r| room_state.revision < r.revision)
                {
                    return;
                }
                if let Some(c) = &mut self.cur {
                    if c.duration_ms.is_none() {
                        c.duration_ms = room_state
                            .queue
                            .iter()
                            .find(|t| t.id == c.track_id)
                            .and_then(|t| t.duration_ms);
                    }
                }
                if self.library.learn_durations(&room_state.queue) {
                    self.playlists_changed();
                }
                self.room = Some(room_state);
                self.reconcile().await;
            }
            ServerMsg::Members { list } => self.view.members = list,
            ServerMsg::Pong {
                client_ms,
                server_ms,
            } => {
                let before = self.clock.offset_ms();
                self.clock.add_sample(client_ms, server_ms, local_ms());
                let after = self.clock.offset_ms();
                if (after - before).abs() > 30 {
                    tracing::debug!(before, after, "clock offset changed");
                }
            }
            ServerMsg::ResolveRequest { request_id, url } => {
                // Usually the lookup already started when the link was submitted.
                match self
                    .adds
                    .iter()
                    .position(|j| j.url == url && j.request_id.is_none())
                {
                    Some(i) => {
                        self.adds[i].request_id = Some(request_id);
                        self.try_finish_add(i);
                    }
                    None => match parse_youtube_url(&url) {
                        Some(r) => {
                            self.start_add_job(r, Some(request_id));
                        }
                        None => self.toast(strings::NOT_YOUTUBE, true),
                    },
                }
            }
            ServerMsg::RadioRequest { request_id, seeds } => self.start_radio(request_id, seeds),
            ServerMsg::Toast { message } => {
                // Older servers announce every add; the queue shows it already.
                if !message.contains(" je dodao/la") {
                    self.toast(message, false)
                }
            }
            ServerMsg::Error { message } => {
                // An error right after `Add` (queue full, too many pending)
                // means the oldest unanswered add will never be resolved.
                if let Some(i) = self.adds.iter().position(|j| j.request_id.is_none()) {
                    if self.adds.remove(i).bulk {
                        self.bulk.clear();
                    }
                }
                self.toast(message, true)
            }
            ServerMsg::Rejected { message } => {
                self.view.conn = ConnState::Rejected(message);
            }
        }
    }

    fn ping_tick(&mut self) {
        if self.cfg.low_latency {
            self.send(ClientMsg::Ping {
                client_ms: local_ms(),
            });
            self.next_ping = Some(Instant::now() + LOW_PING_EVERY);
            return;
        }
        if self.pings_left == 0 {
            self.pings_left = PING_BURST;
        }
        self.send(ClientMsg::Ping {
            client_ms: local_ms(),
        });
        self.pings_left -= 1;
        self.next_ping = Some(
            Instant::now()
                + if self.pings_left > 0 {
                    PING_SPACING
                } else {
                    RESYNC_EVERY
                },
        );
    }

    // ----- background tasks -------------------------------------------------------

    fn start_ytdlp_maintenance(&mut self) {
        if self.cfg.tools.yt_dlp.is_none() {
            return;
        }
        let r = self.resolver.clone();
        let tx = self.task_tx.clone();
        let update = self.cfg.auto_update;
        self.view.ytdlp_updating = update;
        tokio::spawn(async move {
            if update {
                match r.self_update().await {
                    Ok(out) => tracing::info!("yt-dlp -U: {}", out.trim()),
                    Err(e) => tracing::warn!("yt-dlp -U failed: {e}"),
                }
            }
            let v = r.version().await;
            let _ = tx.send(TaskResult::YtdlpVersion(v));
        });
    }

    async fn on_task(&mut self, r: TaskResult) {
        match r {
            TaskResult::YtdlpVersion(v) => {
                self.view.ytdlp_version = v;
                self.view.ytdlp_updating = false;
            }
            TaskResult::Meta { job, result, quick } => {
                if let Some(i) = self.adds.iter().position(|j| j.id == job) {
                    self.adds[i].result = Some((result, quick));
                    self.try_finish_add(i);
                }
            }
            TaskResult::Duration { youtube_id, result } => match result {
                Ok(StreamInfo {
                    duration_ms: Some(d),
                    ..
                }) => self.send(ClientMsg::Duration {
                    youtube_id,
                    duration_ms: d,
                }),
                Ok(_) => {}
                Err(e) => tracing::debug!(youtube_id, "duration lookup failed: {e}"),
            },
            TaskResult::Stream { track_id, result } => self.on_stream(track_id, result).await,
            TaskResult::Prepared { youtube_id, result } => {
                if self.preparing.as_deref() == Some(youtube_id.as_str()) {
                    self.preparing = None;
                }
                match result {
                    Ok(()) => tracing::info!(youtube_id, "next song is on disk"),
                    Err(e) => tracing::debug!(youtube_id, "preparing failed: {e}"),
                }
                // The queue may have moved on meanwhile.
                self.schedule_prepare(Duration::ZERO);
            }
            TaskResult::Search { seq, result } => {
                if seq != self.search_seq {
                    return;
                }
                let s = &mut self.view.search;
                s.loading = false;
                match result {
                    Ok(hits) if hits.is_empty() => s.error = Some(strings::SEARCH_NOTHING.into()),
                    Ok(hits) => s.results = hits,
                    Err(e) => {
                        tracing::warn!("search failed: {e}");
                        s.error = Some(strings::SEARCH_FAILED.into());
                    }
                }
            }
            TaskResult::Import { playlist, result } => {
                self.importing = self.importing.saturating_sub(1);
                self.library_dirty = true;
                match result {
                    Ok(tracks) => self.save_songs(playlist, tracks.into_iter().map(Song::from)),
                    Err(e) => self.toast(format!("{} {e}", strings::RESOLVE_FAILED), true),
                }
            }
            TaskResult::Mix {
                run,
                weight,
                result,
            } => self.on_mix(run, weight, result),
            TaskResult::Radio { request_id, tracks } => {
                tracing::info!(request_id, songs = tracks.len(), "radio answer");
                self.send(ClientMsg::AddResolved { request_id, tracks });
            }
            TaskResult::Loudness { youtube_id, result } => {
                self.measuring.remove(&youtube_id);
                if self.measuring_ahead.as_deref() == Some(youtube_id.as_str()) {
                    self.measuring_ahead = None;
                }
                match result {
                    Ok(l) => {
                        let db = l.gain_db();
                        tracing::info!(youtube_id, gain_db = db, "loudness measured");
                        self.gains.insert(youtube_id.clone(), db);
                        self.loudness_store.insert(&youtube_id, l);
                    }
                    Err(e) if self.measure_retried.insert(youtube_id.clone()) => {
                        // Usually a bad cached URL: get a fresh one now, while
                        // there is time, instead of when the song should start.
                        tracing::warn!(youtube_id, "loudness scan failed, re-resolving: {e}");
                        self.resolver.invalidate(&youtube_id);
                        let current = self
                            .cur
                            .as_ref()
                            .is_some_and(|c| c.youtube_id == youtube_id);
                        if current {
                            self.measure_loudness(&youtube_id, None);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(youtube_id, "loudness scan failed: {e}");
                        self.measure_failed.insert(youtube_id.clone());
                    }
                }
                self.measure_ahead();
                if self
                    .cur
                    .as_ref()
                    .is_some_and(|c| c.youtube_id == youtube_id)
                {
                    self.apply_gain().await;
                    self.release_gain_hold().await;
                }
            }
        }
    }

    // ----- loudness ---------------------------------------------------------------

    /// Starts measuring a song's loudness in the background (once per song).
    /// Without `url` the stream is resolved first (cached by the resolver).
    fn measure_loudness(&mut self, youtube_id: &str, url: Option<String>) {
        if !self.cfg.normalize
            || self.cfg.fake_audio
            || self.gains.contains_key(youtube_id)
            || self.measuring.contains(youtube_id)
            || self.measure_failed.contains(youtube_id)
        {
            return;
        }
        let Some(mpv) = self.cfg.tools.mpv.clone() else {
            return;
        };
        self.measuring.insert(youtube_id.to_string());
        let resolver = self.resolver.clone();
        let tx = self.task_tx.clone();
        let youtube_id = youtube_id.to_string();
        tokio::spawn(async move {
            let url = match url {
                Some(u) => Ok(u),
                None => resolver
                    .stream_info_background(&youtube_id)
                    .await
                    .map(|s| s.url),
            };
            let result = match url {
                Ok(u) => crate::loudness::measure(&mpv, &u).await,
                Err(e) => Err(e),
            };
            let _ = tx.send(TaskResult::Loudness { youtube_id, result });
        });
    }

    /// Sets the player's gain for the current song (0 when off or unknown).
    /// Measures the next few songs (in queue order, one at a time), so they
    /// start right away at the right level.
    fn measure_ahead(&mut self) {
        if !self.cfg.normalize || self.cfg.fake_audio || self.measuring_ahead.is_some() {
            return;
        }
        // Not while a song is starting: it gets the bandwidth first.
        if self
            .cur
            .as_ref()
            .is_some_and(|c| c.phase != CurPhase::Active)
        {
            return;
        }
        let Some(room) = &self.room else { return };
        let Some(i) = room.playback.track_index else {
            return;
        };
        let next = room
            .queue
            .iter()
            .skip(i + 1)
            .take(MEASURE_AHEAD)
            .map(|t| &t.youtube_id)
            .find(|id| {
                !self.gains.contains_key(*id)
                    && !self.measuring.contains(*id)
                    && !self.measure_failed.contains(*id)
            })
            .cloned();
        if let Some(id) = next {
            self.measure_loudness(&id, None);
            if self.measuring.contains(&id) {
                self.measuring_ahead = Some(id);
            }
        }
    }

    async fn apply_gain(&mut self) {
        let gain = match &self.cur {
            Some(c) if self.cfg.normalize => Some(
                self.gains
                    .get(&c.youtube_id)
                    .copied()
                    .unwrap_or_else(|| typical_gain(&self.gains)),
            ),
            _ => None,
        };
        if let Some(p) = &self.player {
            p.set_gain(gain.unwrap_or(0.0)).await;
        }
    }

    fn gain_hold_deadline(&self) -> Option<Instant> {
        self.cur
            .as_ref()
            .filter(|c| c.phase == CurPhase::AwaitGain)
            .and_then(|c| c.gain_deadline)
    }

    /// Lets a song that waited for its loudness continue (measured or not).
    async fn release_gain_hold(&mut self) {
        let Some(cur) = &mut self.cur else { return };
        cur.gain_deadline = None;
        if cur.phase != CurPhase::AwaitGain {
            return;
        }
        cur.phase = CurPhase::Active;
        self.continue_positioned().await;
        self.measure_ahead();
        self.schedule_prepare(PREPARE_DELAY);
    }

    // ----- equalizer and video ----------------------------------------------------

    async fn set_eq(&mut self, eq: crate::eq::EqSettings) {
        let old = std::mem::replace(&mut self.cfg.eq, eq.clone());
        let Some(p) = &self.player else { return };
        if old.enabled != eq.enabled {
            p.set_af(&crate::eq::af_value(&eq)).await;
            return;
        }
        if !eq.enabled {
            return;
        }
        // Same filter, new gains: changed in place (no rebuild, no dropout).
        for (i, (a, b)) in old.gains.iter().zip(eq.gains.iter()).enumerate() {
            if (a - b).abs() > 0.01 {
                p.eq_command(&crate::eq::band_target(i), "g", &format!("{b:.1}"))
                    .await;
            }
        }
        if (old.preamp_db() - eq.preamp_db()).abs() > 0.01 {
            p.eq_command(
                crate::eq::PREAMP_TARGET,
                "volume",
                &format!("{:.1}dB", eq.preamp_db()),
            )
            .await;
        }
    }

    async fn set_video(&mut self, on: bool) {
        self.cfg.video = on;
        let Some(p) = &self.player else { return };
        p.set_video(on).await;
        if on && self.cur.is_some() {
            // Turned on mid-song: load the song again, now with its video
            // (adding a video track to a playing song crashes mpv).
            self.unload().await;
            self.reconcile().await;
        }
    }

    // ----- next song on disk --------------------------------------------------------

    fn schedule_prepare(&mut self, delay: Duration) {
        if self.cache.is_none() || self.prepare_at.is_some() || self.preparing.is_some() {
            return;
        }
        let active = self
            .cur
            .as_ref()
            .is_some_and(|c| c.phase == CurPhase::Active);
        if active && self.next_to_prepare().is_some() {
            self.prepare_at = Some(Instant::now() + delay);
        }
    }

    /// The next song in the queue, if it is not on disk yet.
    fn next_to_prepare(&self) -> Option<String> {
        let cache = self.cache.as_ref()?;
        let room = self.room.as_ref()?;
        let i = room.playback.track_index?;
        let next = room.queue.get(i + 1)?;
        let id = &next.youtube_id;
        (cache.path(id).is_none() && !cache.busy(id)).then(|| id.clone())
    }

    // ----- shuffle ---------------------------------------------------------------

    /// Picks a random order for the songs after the current one. The server
    /// only knows single moves, so the songs are then moved one at a time,
    /// each right after the one placed before it.
    fn start_shuffle(&mut self) {
        if self.shuffle.take().is_some() || self.clear.is_some() {
            return;
        }
        let Some(room) = &self.room else {
            return;
        };
        let from = room.playback.track_index.map_or(0, |i| i + 1);
        let mut order: Vec<Uuid> = room.queue.iter().skip(from).map(|t| t.id).collect();
        if order.len() < 2 {
            return;
        }
        for i in (1..order.len()).rev() {
            let j = (Uuid::new_v4().as_u128() % (i as u128 + 1)) as usize;
            order.swap(i, j);
        }
        self.shuffle = Some(Shuffle {
            order,
            done: 0,
            prev: None,
            sent: None,
        });
    }

    fn shuffle_at(&self) -> Option<Instant> {
        let s = self.shuffle.as_ref()?;
        Some(self.paced(s.sent))
    }

    /// When the next message of a paced series may go out: shortly after
    /// the server answered the last one, or after a wait when it did not.
    fn paced(&self, sent: Option<(u64, Instant)>) -> Instant {
        let Some((rev, at)) = sent else {
            return Instant::now();
        };
        let answered = self.room.as_ref().is_some_and(|r| r.revision > rev);
        if answered {
            at + SHUFFLE_GAP
        } else {
            at + SHUFFLE_WAIT
        }
    }

    // ----- clear -------------------------------------------------------------------

    /// Empties the queue for everyone (a second call stops it).
    fn start_clear(&mut self) {
        if self.clear.take().is_some() {
            return;
        }
        let Some(room) = &self.room else {
            return;
        };
        let current = room.current_track().map(|t| t.id);
        let mut ids: Vec<Uuid> = room
            .queue
            .iter()
            .rev()
            .map(|t| t.id)
            .filter(|id| Some(*id) != current)
            .collect();
        ids.extend(current);
        if ids.is_empty() {
            return;
        }
        self.shuffle = None;
        self.clear = Some(Clear {
            ids,
            done: 0,
            sent: None,
        });
    }

    fn clear_at(&self) -> Option<Instant> {
        let c = self.clear.as_ref()?;
        Some(self.paced(c.sent))
    }

    /// Sends the next removal of a clear.
    fn clear_step(&mut self) {
        let (Some(room), Some(c)) = (&self.room, &mut self.clear) else {
            self.clear = None;
            return;
        };
        while c.done < c.ids.len() {
            let id = c.ids[c.done];
            c.done += 1;
            if room.queue.iter().any(|t| t.id == id) {
                c.sent = Some((room.revision, Instant::now()));
                self.send(ClientMsg::Remove { track_id: id });
                return;
            }
        }
        self.clear = None;
        self.toast(strings::QUEUE_CLEARED, false);
    }

    /// Sends the next move of a shuffle, based on the room as it is now.
    fn shuffle_step(&mut self) {
        let (Some(room), Some(s)) = (&self.room, &mut self.shuffle) else {
            self.shuffle = None;
            return;
        };
        let pos = |id: Uuid| room.queue.iter().position(|t| t.id == id);
        while s.done < s.order.len() {
            let id = s.order[s.done];
            s.done += 1;
            let Some(idx) = pos(id) else {
                continue;
            };
            let anchor = s.prev.and_then(pos).or(room.playback.track_index);
            s.prev = Some(id);
            let target = anchor.map_or(0, |a| a + 1);
            if idx == target {
                continue;
            }
            // The server takes the song out first, then inserts it.
            let new_index = if idx < target { target - 1 } else { target };
            s.sent = Some((room.revision, Instant::now()));
            let msg = ClientMsg::Move {
                track_id: id,
                new_index,
            };
            self.send(msg);
            return;
        }
        self.shuffle = None;
        self.toast(strings::SHUFFLED, false);
    }

    fn prepare_next(&mut self) {
        self.prepare_at = None;
        if self.preparing.is_some()
            || !self
                .cur
                .as_ref()
                .is_some_and(|c| c.phase == CurPhase::Active)
        {
            return;
        }
        let (Some(cache), Some(id)) = (self.cache.clone(), self.next_to_prepare()) else {
            return;
        };
        self.preparing = Some(id.clone());
        let resolver = self.resolver.clone();
        let tx = self.task_tx.clone();
        tokio::spawn(async move {
            let result = match resolver.stream_info_background(&id).await {
                Ok(info) => {
                    let (c, i) = (cache, id.clone());
                    tokio::task::spawn_blocking(move || c.download(&i, &info.url))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                }
                Err(e) => Err(e),
            };
            let _ = tx.send(TaskResult::Prepared {
                youtube_id: id,
                result,
            });
        });
    }

    // ----- player -----------------------------------------------------------------

    async fn ensure_player(&mut self) {
        if self.player.is_some() {
            return;
        }
        if self.cfg.fake_audio {
            self.player = Some(Player::Fake(FakePlayer::new(
                self.player_tx.clone(),
                self.cfg.fake_rate,
                Duration::from_millis(400),
            )));
            return;
        }
        let Some(mpv) = self.cfg.tools.mpv.clone() else {
            self.view.player_error = Some(strings::MPV_MISSING.into());
            return;
        };
        let opts = crate::mpv::SpawnOpts {
            volume: self.view.volume,
            video: self.cfg.video,
            af: crate::eq::af_value(&self.cfg.eq),
            wid: self.cfg.video_wid,
        };
        match crate::mpv::Mpv::spawn(&mpv, &opts, self.player_tx.clone()).await {
            Ok(m) => {
                self.view.player_error = None;
                self.player = Some(Player::Mpv(Box::new(m)));
            }
            Err(e) => {
                tracing::error!("cannot start mpv: {e}");
                self.view.player_error = Some(e);
            }
        }
    }

    /// Stops playback and forgets the current track.
    async fn unload(&mut self) {
        if self.cur.take().is_some() {
            if let Some(p) = &self.player {
                p.stop().await;
                p.set_speed(1.0).await;
            }
        }
        self.applied_version = None;
        self.unpause_at = None;
        self.next_drift = None;
        self.view.drift_ms = None;
        self.view.player_pos_ms = None;
    }

    async fn on_player(&mut self, ev: PlayerEvent) {
        match ev {
            PlayerEvent::Positioned => self.on_positioned().await,
            PlayerEvent::Eof { entry } => {
                let Some(cur) = &self.cur else { return };
                if cur.entry != entry || cur.phase != CurPhase::Active {
                    return;
                }
                if let Some(v) = self.room.as_ref().map(|r| r.playback.version) {
                    if self.ended_sent != Some(v) {
                        self.ended_sent = Some(v);
                        tracing::info!(version = v, "track ended");
                        self.send(ClientMsg::Ended { version: v });
                    }
                }
                self.next_drift = None;
            }
            PlayerEvent::Failed { entry, error } => {
                let Some(cur) = &mut self.cur else { return };
                if cur.entry != entry {
                    return;
                }
                tracing::warn!(id = %cur.youtube_id, "playback failed: {error}");
                if cur.local {
                    // A bad cached file: drop it and stream instead.
                    cur.local = false;
                    cur.phase = CurPhase::Resolving;
                    let (id, track_id) = (cur.youtube_id.clone(), cur.track_id);
                    if let Some(c) = &self.cache {
                        c.forget(&id);
                    }
                    self.spawn_resolve(track_id, id);
                } else if !cur.reresolved {
                    // The cached URL may have expired: resolve again once.
                    cur.reresolved = true;
                    cur.phase = CurPhase::Resolving;
                    let (id, track_id) = (cur.youtube_id.clone(), cur.track_id);
                    self.resolver.invalidate(&id);
                    self.spawn_resolve(track_id, id);
                } else {
                    self.give_up_track().await;
                }
            }
            PlayerEvent::VideoClosed => {
                tracing::info!("video window closed: video mode off");
                self.set_video(false).await;
                self.view.video_closed += 1;
            }
            PlayerEvent::Crashed => {
                tracing::error!("player crashed");
                self.player = None;
                self.cur = None;
                self.applied_version = None;
                self.unpause_at = None;
                self.next_drift = None;
                self.player_restarts += 1;
                if self.player_restarts <= 5 {
                    self.toast(strings::MPV_RESTARTED, true);
                    self.ensure_player().await;
                    self.reconcile().await;
                }
            }
        }
    }

    /// Tells the group to move on from a track this client cannot play.
    async fn give_up_track(&mut self) {
        let title = self
            .room
            .as_ref()
            .and_then(|r| r.current_track())
            .map(|t| t.title.clone())
            .unwrap_or_default();
        self.toast(format!("{} {title}", strings::SKIPPING_BROKEN), true);
        if let Some(c) = &mut self.cur {
            c.phase = CurPhase::Failed;
        }
        if let Some(v) = self.room.as_ref().map(|r| r.playback.version) {
            if self.ended_sent != Some(v) {
                self.ended_sent = Some(v);
                self.send(ClientMsg::Ended { version: v });
            }
        }
    }

    // ----- adding links ------------------------------------------------------------

    /// Registers a submitted link and starts resolving it immediately (in
    /// parallel with the server round trip). Returns the canonical URL.
    fn start_add_job(&mut self, r: YoutubeRef, request_id: Option<u64>) -> String {
        self.next_add_id += 1;
        let id = self.next_add_id;
        let (url, playlist) = match &r {
            YoutubeRef::Video(v) => (watch_url(v), false),
            YoutubeRef::Playlist(p) => (playlist_url(p), true),
        };
        self.adds.push(AddJob {
            id,
            url: url.clone(),
            playlist,
            request_id,
            result: None,
            started: Instant::now(),
            bulk: false,
        });
        let resolver = self.resolver.clone();
        let tx = self.task_tx.clone();
        let meta_url = url.clone();
        tokio::spawn(async move {
            let (result, quick) = match &r {
                YoutubeRef::Video(v) => match resolver.quick_video(v).await {
                    Ok(t) => (Ok(vec![t]), true),
                    Err(e) => {
                        if !resolver.is_fake() {
                            tracing::debug!(v, "quick lookup failed, using yt-dlp: {e}");
                        }
                        (resolver.metadata(&meta_url).await, false)
                    }
                },
                YoutubeRef::Playlist(_) => (resolver.metadata(&meta_url).await, false),
            };
            let _ = tx.send(TaskResult::Meta {
                job: id,
                result,
                quick,
            });
        });
        url
    }

    /// Sends `AddResolved` once both the server's request id and the
    /// metadata are known.
    fn try_finish_add(&mut self, i: usize) {
        let job = &self.adds[i];
        let (Some(request_id), Some(_)) = (job.request_id, &job.result) else {
            return;
        };
        let job = self.adds.remove(i);
        let Some((result, quick)) = job.result else {
            return;
        };
        match result {
            Ok(tracks) => {
                if quick {
                    // Learn the duration (and warm the stream cache) in the background.
                    for t in tracks.iter().filter(|t| t.duration_ms.is_none()) {
                        let resolver = self.resolver.clone();
                        let tx = self.task_tx.clone();
                        let youtube_id = t.youtube_id.clone();
                        tokio::spawn(async move {
                            let result = resolver.stream_info_background(&youtube_id).await;
                            let _ = tx.send(TaskResult::Duration { youtube_id, result });
                        });
                    }
                }
                self.send(ClientMsg::AddResolved { request_id, tracks });
            }
            Err(e) => self.toast(format!("{} {e}", strings::RESOLVE_FAILED), true),
        }
        if job.bulk {
            self.send_batch();
        }
    }

    /// Sends the next batch of a playlist, unless one is on its way. The
    /// server only takes links, so the batch goes as the answer to its first
    /// song's link.
    fn send_batch(&mut self) {
        if self.adds.iter().any(|j| j.bulk) {
            return;
        }
        let Some(tracks) = self.bulk.pop_front() else {
            return;
        };
        let Some(first) = tracks.first() else { return };
        let url = watch_url(&first.youtube_id);
        self.next_add_id += 1;
        self.adds.push(AddJob {
            id: self.next_add_id,
            url: url.clone(),
            playlist: true,
            request_id: None,
            result: Some((Ok(tracks), false)),
            started: Instant::now(),
            bulk: true,
        });
        self.send(ClientMsg::Add { url });
    }

    // ----- the user's playlists, history and recommendations ----------------------------

    fn publish_library(&mut self) {
        if !std::mem::take(&mut self.library_dirty) {
            return;
        }
        self.view.library = Arc::new(LibraryView {
            playlists: self.library.playlists().to_vec(),
            recs: self.library.recs().to_vec(),
            recs_loading: self.rec_run.is_some(),
            recs_error: self.recs_error.clone(),
            has_history: self.library.has_history(),
            importing: self.importing,
        });
    }

    fn playlists_changed(&mut self) {
        self.library_dirty = true;
        if let Err(e) = self.library.save_playlists() {
            self.toast(format!("{} {e}", strings::PLAYLIST_SAVE_FAILED), true);
        }
    }

    fn history_changed(&mut self) {
        self.library_dirty = true;
        if let Err(e) = self.library.save_history() {
            tracing::warn!("cannot save history: {e}");
        }
    }

    fn save(&mut self, to: SaveTo, item: SaveItem) {
        let id = match to {
            SaveTo::Existing(id) => id,
            SaveTo::New(name) => match self.library.create(&name) {
                Some(id) => id,
                None => return,
            },
        };
        match item {
            SaveItem::Songs(songs) => self.save_songs(id, songs),
            SaveItem::YoutubePlaylist(url) => {
                self.playlists_changed();
                self.importing += 1;
                let resolver = self.resolver.clone();
                let tx = self.task_tx.clone();
                tokio::spawn(async move {
                    let result = resolver.metadata(&url).await;
                    let _ = tx.send(TaskResult::Import {
                        playlist: id,
                        result,
                    });
                });
            }
        }
    }

    fn save_songs(&mut self, id: Uuid, songs: impl IntoIterator<Item = Song>) {
        let Some(added) = self.library.add(id, songs) else {
            return;
        };
        self.playlists_changed();
        if let Some(p) = self.library.get(id) {
            let text = strings::saved_to(&p.name, added.added, added.no_room);
            self.toast(text, added.no_room > 0);
        }
    }

    /// Remembers the playing song once enough of it was heard.
    fn count_listening(&mut self) {
        if self.cfg.library_dir.is_none() {
            return;
        }
        let playing = self.phase() == Phase::Playing;
        let (track, duration) = match &self.cur {
            Some(c) => (Some(c.track_id), c.duration_ms),
            None => (None, None),
        };
        if !self.listen.tick(track, playing, duration, Instant::now()) {
            return;
        }
        let Some(t) = self
            .room
            .as_ref()
            .and_then(|r| r.queue.iter().find(|t| Some(t.id) == track))
        else {
            return;
        };
        let own = self.view.me.as_ref().is_some_and(|m| m.name == t.added_by);
        let song = Song::from(t);
        tracing::info!(id = song.youtube_id, own, "listened");
        self.library.record(&song, own, library::unix_now());
        self.history_changed();
    }

    /// Starts looking for similar songs (no-op while already looking).
    fn start_recs(&mut self) {
        if self.rec_run.is_some() {
            return;
        }
        let queue = self.room.as_ref().map_or(&[][..], |r| &r.queue);
        let random = || (Uuid::new_v4().as_u128() >> 64) as f64 / u64::MAX as f64;
        let seeds = self.library.seeds(queue, library::unix_now(), random);
        self.recs_error = None;
        self.library_dirty = true;
        if seeds.is_empty() {
            return;
        }
        self.rec_seq += 1;
        self.rec_run = Some(RecRun {
            seeds,
            asked: 0,
            mixes: Vec::new(),
        });
        self.next_mix();
    }

    /// Asks for the next seed's mix, or finishes the run.
    fn next_mix(&mut self) {
        let Some(r) = &mut self.rec_run else { return };
        if let Some((song, weight)) = r.seeds.get(r.asked).cloned() {
            r.asked += 1;
            let run = self.rec_seq;
            let resolver = self.resolver.clone();
            let tx = self.task_tx.clone();
            tokio::spawn(async move {
                let result = resolver.mix(&song.youtube_id).await;
                let _ = tx.send(TaskResult::Mix {
                    run,
                    weight,
                    result,
                });
            });
            return;
        }
        if r.mixes.is_empty() {
            self.recs_error = Some(strings::RECS_FAILED.into());
        }
        self.rec_run = None;
        self.history_changed();
    }

    /// Radio: looks up YouTube's mix of the newest seed (older seeds when
    /// that finds nothing new) and answers the server with the songs that
    /// aren't in the queue yet.
    fn start_radio(&mut self, request_id: u64, seeds: Vec<String>) {
        let queued: HashSet<String> = self
            .room
            .as_ref()
            .map(|r| r.queue.iter().map(|t| t.youtube_id.clone()).collect())
            .unwrap_or_default();
        let resolver = self.resolver.clone();
        let tx = self.task_tx.clone();
        tokio::spawn(async move {
            let mut tracks = Vec::new();
            for seed in seeds.iter().rev() {
                match resolver.mix(seed).await {
                    Ok(hits) => tracks = radio_pick(hits, &queued),
                    Err(e) => tracing::warn!(seed, "radio mix failed: {e}"),
                }
                if !tracks.is_empty() {
                    break;
                }
            }
            let _ = tx.send(TaskResult::Radio { request_id, tracks });
        });
    }

    fn on_mix(&mut self, run: u64, weight: f64, result: Result<Vec<SearchHit>, String>) {
        if run != self.rec_seq {
            return;
        }
        let Some(r) = &mut self.rec_run else { return };
        match result {
            Ok(hits) => {
                let songs = hits.into_iter().map(|h| Song {
                    youtube_id: h.youtube_id,
                    title: h.title,
                    duration_ms: h.duration_ms,
                    channel: h.channel,
                });
                r.mixes.push((weight, songs.collect()));
                let queue = self.room.as_ref().map_or(&[][..], |r| &r.queue);
                let now = library::unix_now();
                let items = self.library.rank(&r.seeds, &r.mixes, queue, now);
                self.library.set_recs(items, now);
                self.library_dirty = true;
            }
            Err(e) => tracing::warn!("mix failed: {e}"),
        }
        self.next_mix();
    }

    fn spawn_resolve(&self, track_id: Uuid, youtube_id: String) {
        let r = self.resolver.clone();
        let tx = self.task_tx.clone();
        tokio::spawn(async move {
            let result = r.stream_info(&youtube_id).await;
            let _ = tx.send(TaskResult::Stream { track_id, result });
        });
    }

    async fn on_stream(&mut self, track_id: Uuid, result: Result<StreamInfo, String>) {
        let Some(cur) = &mut self.cur else { return };
        if cur.track_id != track_id || cur.phase != CurPhase::Resolving {
            return;
        }
        let since = self.track_t0.map(|t| t.elapsed().as_millis() as u64);
        tracing::info!(since_ms = since, "stream resolved");
        let Some(cur) = &mut self.cur else { return };
        let cached = self.cache.as_ref().and_then(|c| c.path(&cur.youtube_id));
        let url = match result {
            Ok(info) => {
                cur.video_url = info.video_url.clone();
                if let (None, Some(d)) = (cur.duration_ms, info.duration_ms) {
                    // Track was quick-added without a duration: tell everyone.
                    cur.duration_ms = Some(d);
                    let youtube_id = cur.youtube_id.clone();
                    self.send(ClientMsg::Duration {
                        youtube_id,
                        duration_ms: d,
                    });
                }
                info.url
            }
            Err(_) if cached.is_some() => String::new(),
            Err(e) => {
                tracing::warn!("resolve failed twice: {e}");
                self.give_up_track().await;
                return;
            }
        };
        let (url, local) = match cached {
            Some(p) => (p.to_string_lossy().into_owned(), true),
            None => (url, false),
        };
        if let Some(c) = &mut self.cur {
            c.local = local;
        }
        let comp = if local {
            self.load_comp_local_ms
        } else {
            self.load_comp_ms
        };
        self.ensure_player().await;
        let Some(room) = self.room.clone() else {
            return;
        };
        let pb = &room.playback;
        let start = match pb.status {
            PlayStatus::Playing if self.server_now() >= pb.anchor_server_ms => {
                // Late joiner: aim where the timeline will be once loaded.
                pb.position_at(self.server_now()) + comp
            }
            _ => pb.anchor_position_ms,
        };
        let duration = self.cur.as_ref().and_then(|c| c.duration_ms);
        let start = match duration {
            Some(d) => start.min(d.saturating_sub(1000)),
            None => start,
        };
        let youtube_id = self.cur.as_ref().map(|c| c.youtube_id.clone());
        if let Some(id) = youtube_id {
            self.measure_loudness(&id, Some(url.clone()));
            let pending = self.measuring.contains(&id);
            if let Some(c) = &mut self.cur {
                c.gain_deadline = pending.then(|| Instant::now() + GAIN_WAIT);
            }
        }
        self.apply_gain().await;
        let title = room
            .current_track()
            .map(|t| t.title.clone())
            .unwrap_or_default();
        let video = self
            .cur
            .as_ref()
            .filter(|_| self.cfg.video)
            .and_then(|c| c.video_url.clone());
        let Some(player) = &self.player else { return };
        match player
            .load(&url, start, duration, &title, video.as_deref())
            .await
        {
            Ok(entry) => {
                if let Some(c) = &mut self.cur {
                    c.entry = entry;
                    c.phase = CurPhase::Loading;
                    c.paused = true;
                    c.speed = 1.0;
                    c.video_loaded = video.is_some();
                }
                self.op_started = Some(Instant::now());
                tracing::info!(start, local, "loading track");
            }
            Err(e) => {
                tracing::warn!("load failed: {e}");
                self.give_up_track().await;
            }
        }
    }

    async fn on_positioned(&mut self) {
        let Some(cur) = &mut self.cur else { return };
        let took = self
            .op_started
            .take()
            .map(|t| t.elapsed().as_millis() as u64);
        match cur.phase {
            CurPhase::Loading => {
                if let Some(ms) = took {
                    if cur.local {
                        self.load_comp_local_ms = ewma(self.load_comp_local_ms, ms.clamp(50, 8000));
                        tracing::info!(ms, comp = self.load_comp_local_ms, "track loaded (disk)");
                    } else if !cur.video_loaded {
                        // Loads with a video track take longer; keep them
                        // out of the audio-only estimate.
                        self.load_comp_ms = ewma(self.load_comp_ms, ms.clamp(200, 8000));
                        tracing::info!(ms, comp = self.load_comp_ms, "track loaded");
                    }
                }
            }
            CurPhase::Seeking => {
                if let Some(ms) = took {
                    self.seek_comp_ms = ewma(self.seek_comp_ms, ms.clamp(20, 5000));
                }
            }
            _ => return,
        }
        let hold =
            cur.phase == CurPhase::Loading && cur.gain_deadline.is_some_and(|t| Instant::now() < t);
        if hold {
            cur.phase = CurPhase::AwaitGain;
            tracing::debug!("holding until loudness is known");
            return;
        }
        cur.phase = CurPhase::Active;
        self.continue_positioned().await;
        self.measure_ahead();
        self.schedule_prepare(PREPARE_DELAY);
    }

    /// After a load or seek finished: report ready or join the timeline.
    async fn continue_positioned(&mut self) {
        let version = self.room.as_ref().map(|r| r.playback.version);
        if self.applied_version != version {
            self.reconcile().await;
            return;
        }
        match self.room.as_ref().map(|r| r.playback.status) {
            Some(PlayStatus::Paused) => self.send_ready(),
            Some(PlayStatus::Playing) => self.align_playing().await,
            _ => {}
        }
    }

    fn send_ready(&mut self) {
        let Some(v) = self.room.as_ref().map(|r| r.playback.version) else {
            return;
        };
        if self.ready_sent != Some(v) {
            self.ready_sent = Some(v);
            let since = self.track_t0.map(|t| t.elapsed().as_millis() as u64);
            tracing::info!(since_ms = since, "ready sent");
            self.send(ClientMsg::Ready { version: v });
        }
    }

    // ----- sync -------------------------------------------------------------------

    /// Brings the player in line with the room state.
    async fn reconcile(&mut self) {
        let Some(room) = self.room.clone() else {
            self.unload().await;
            return;
        };
        let Some(track) = room.current_track().cloned() else {
            self.unload().await;
            return;
        };
        if room.playback.status == PlayStatus::Stopped {
            self.unload().await;
            return;
        }
        // Measure upcoming songs ahead, so they start at the right level.
        self.measure_ahead();
        self.schedule_prepare(PREPARE_DELAY);
        // The next song's URL is ready before it is needed (the queue can
        // change at any time, so this is checked on every update).
        if let Some(next) = room
            .playback
            .track_index
            .and_then(|i| room.queue.get(i + 1))
        {
            self.resolver.prefetch(&next.youtube_id);
        }
        let same = self.cur.as_ref().is_some_and(|c| c.track_id == track.id);
        if !same {
            self.unload().await;
            self.ended_sent = None;
            self.ready_sent = None;
            self.align_attempts = 0;
            self.track_t0 = Some(Instant::now());
            self.cur = Some(Cur {
                track_id: track.id,
                youtube_id: track.youtube_id.clone(),
                duration_ms: track.duration_ms,
                phase: CurPhase::Resolving,
                entry: 0,
                paused: true,
                speed: 1.0,
                reresolved: false,
                gain_deadline: None,
                video_url: None,
                video_loaded: false,
                local: false,
            });
            let on_disk = self
                .cache
                .as_ref()
                .and_then(|c| c.path(&track.youtube_id))
                .is_some();
            if on_disk && !self.cfg.video {
                // Nothing to wait for: load from disk right away.
                let _ = self.task_tx.send(TaskResult::Stream {
                    track_id: track.id,
                    result: Err(String::new()),
                });
            } else {
                self.spawn_resolve(track.id, track.youtube_id.clone());
            }
            return;
        }
        let phase = self.cur.as_ref().map(|c| c.phase);
        if phase == Some(CurPhase::Failed) {
            // Version changed but same broken track (e.g. seek): tell again.
            if self.applied_version != Some(room.playback.version) {
                self.applied_version = Some(room.playback.version);
                self.give_up_track().await;
            }
            return;
        }
        if phase != Some(CurPhase::Active) {
            return;
        }
        if self.applied_version == Some(room.playback.version) {
            return;
        }
        self.applied_version = Some(room.playback.version);
        self.align_attempts = 0;
        self.unpause_at = None;
        match room.playback.status {
            PlayStatus::Paused => {
                self.next_drift = None;
                self.set_paused(true).await;
                self.set_speed(1.0).await;
                let pos = self.position().await.map(|(p, _)| p);
                let target = room.playback.anchor_position_ms;
                if pos.is_none_or(|p| p.abs_diff(target) > 30) {
                    self.seek(target).await;
                } else {
                    self.send_ready();
                }
            }
            PlayStatus::Playing => self.align_playing().await,
            PlayStatus::Stopped => {}
        }
    }

    /// Gets the player to play in sync with the timeline: waits (paused) if
    /// it is ahead, unpauses if slightly behind, seeks if far off.
    async fn align_playing(&mut self) {
        let Some(room) = self.room.clone() else {
            return;
        };
        let pb = &room.playback;
        let Some((pos, at_local)) = self.position().await else {
            return;
        };
        let server_at = self.timeline_at(at_local) as i64;
        let reach = pb.anchor_server_ms as i64 + (pos as i64 - pb.anchor_position_ms as i64);
        let wait = reach - server_at;
        let paused = self.cur.as_ref().is_none_or(|c| c.paused);
        tracing::debug!(pos, wait, paused, "align");
        // Within these the drift check fixes it with the speed (no pause/seek).
        let (ahead_ok, behind_ok) = if self.cfg.low_latency {
            (shared::sync::LOW_SEEK_MS, shared::sync::LOW_SEEK_MS)
        } else {
            (shared::sync::DRIFT_IGNORE_MS, shared::sync::DRIFT_SEEK_MS)
        };
        if (0..=MAX_WAIT_MS).contains(&wait) {
            if !paused {
                if wait <= ahead_ok {
                    self.schedule_drift();
                    return;
                }
                self.set_paused(true).await;
            }
            self.set_speed(1.0).await;
            if wait < 2 {
                self.unpause_now().await;
            } else {
                self.unpause_at = Some(instant_at_local_ms(at_local + wait as u64));
            }
        } else if wait < 0 && -wait <= behind_ok {
            if paused {
                self.unpause_now().await;
            } else {
                self.schedule_drift();
            }
        } else {
            self.align_attempts += 1;
            if self.align_attempts > 3 {
                tracing::warn!("cannot align after 3 seeks, playing anyway");
                self.unpause_now().await;
                return;
            }
            let now = self.timeline_at(local_ms());
            let target = if now < pb.anchor_server_ms {
                pb.anchor_position_ms
            } else {
                pb.position_at(now) + self.seek_comp_ms
            };
            let target = match self.cur.as_ref().and_then(|c| c.duration_ms) {
                Some(d) => target.min(d.saturating_sub(500)),
                None => target,
            };
            self.set_paused(true).await;
            self.set_speed(1.0).await;
            self.seek(target).await;
        }
    }

    async fn unpause_now(&mut self) {
        self.unpause_at = None;
        let playing = self
            .room
            .as_ref()
            .is_some_and(|r| r.playback.status == PlayStatus::Playing);
        let active = self
            .cur
            .as_ref()
            .is_some_and(|c| c.phase == CurPhase::Active);
        if playing && active {
            self.set_paused(false).await;
            self.schedule_drift();
        }
    }

    fn schedule_drift(&mut self) {
        let every = if self.cfg.low_latency {
            LOW_DRIFT_EVERY
        } else {
            DRIFT_EVERY
        };
        self.next_drift = Some(Instant::now() + every);
    }

    /// The room's timeline at a local time, as this device should be
    /// playing it: shifted by how late its sound comes out.
    fn timeline_at(&self, at_local: u64) -> u64 {
        let t = to_server_ms(at_local, self.clock.offset_ms()) as i64 + self.cfg.audio_offset_ms;
        t.max(0) as u64
    }

    async fn drift_check(&mut self) {
        self.next_drift = None;
        let Some(room) = self.room.clone() else {
            return;
        };
        let pb = &room.playback;
        let ok = pb.status == PlayStatus::Playing
            && self.unpause_at.is_none()
            && self
                .cur
                .as_ref()
                .is_some_and(|c| c.phase == CurPhase::Active && !c.paused);
        if !ok {
            return;
        }
        let Some((pos, at_local)) = self.position().await else {
            self.schedule_drift();
            return;
        };
        let server_at = self.timeline_at(at_local);
        if server_at < pb.anchor_server_ms {
            self.schedule_drift();
            return;
        }
        let duration = self.cur.as_ref().and_then(|c| c.duration_ms);
        let expected = expected_position_clamped(pb, server_at, duration);
        if duration.is_some_and(|d| expected + 1500 >= d) {
            // Near the end: let it finish naturally.
            return;
        }
        let drift = pos as i64 - expected as i64;
        self.view.drift_ms = Some(drift);
        self.view.player_pos_ms = Some(pos);
        let speed = self.cur.as_ref().map_or(1.0, |c| c.speed);
        let action = if self.cfg.low_latency {
            decide_drift_low(drift)
        } else {
            decide_drift(drift, speed)
        };
        match action {
            DriftAction::None => self.schedule_drift(),
            DriftAction::SetSpeed(s) => {
                tracing::debug!(drift, speed = s, "nudging speed");
                self.set_speed(s).await;
                self.schedule_drift();
            }
            DriftAction::Seek => {
                tracing::info!(drift, "drift too large, re-aligning");
                self.align_attempts = 0;
                self.align_playing().await;
            }
        }
    }

    // ----- player helpers -------------------------------------------------------------

    async fn position(&self) -> Option<(u64, u64)> {
        self.player.as_ref()?.position().await
    }

    async fn set_paused(&mut self, paused: bool) {
        if let (Some(p), Some(c)) = (&self.player, &mut self.cur) {
            if c.paused != paused {
                p.set_paused(paused).await;
                c.paused = paused;
            }
        }
    }

    async fn set_speed(&mut self, speed: f64) {
        if let (Some(p), Some(c)) = (&self.player, &mut self.cur) {
            if (c.speed - speed).abs() > 1e-6 {
                p.set_speed(speed).await;
                c.speed = speed;
            }
        }
    }

    async fn seek(&mut self, pos_ms: u64) {
        if let (Some(p), Some(c)) = (&self.player, &mut self.cur) {
            p.seek(pos_ms).await;
            c.phase = CurPhase::Seeking;
            self.op_started = Some(Instant::now());
            self.next_drift = None;
        }
    }
}

#[cfg(test)]
mod radio_tests {
    use super::*;

    fn hit(id: &str) -> SearchHit {
        SearchHit {
            youtube_id: id.into(),
            title: id.into(),
            channel: None,
            duration_ms: Some(1000),
            playlist: false,
            thumb_id: None,
            count: None,
        }
    }

    #[test]
    fn radio_pick_skips_queued_and_repeats() {
        let queued: HashSet<String> = ["a".to_string()].into();
        let hits = ["a", "b", "b", "c", "d", "e", "f", "g"].map(hit).to_vec();
        let ids: Vec<String> = radio_pick(hits, &queued)
            .into_iter()
            .map(|t| t.youtube_id)
            .collect();
        assert_eq!(ids, ["b", "c", "d", "e", "f"]);
        assert!(radio_pick(vec![hit("a")], &queued).is_empty());
    }
}
