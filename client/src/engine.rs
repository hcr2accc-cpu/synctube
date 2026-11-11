//! The client core: connection, clock sync, stream resolving and the sync
//! loop that keeps the local player on the shared timeline. Runs on its own
//! thread with a single-threaded tokio runtime; the UI talks to it through
//! a command channel and reads a shared `View` snapshot.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shared::sync::{decide_drift, expected_position_clamped, to_server_ms, DriftAction};
use shared::{ClientMsg, Member, PlayStatus, RoomState, ServerMsg, TrackInfo};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::clock::{instant_at_local_ms, local_ms, ClockSync};
use crate::net::{self, NetEvent, NetHandle};
use crate::paths::Tools;
use crate::player::{FakePlayer, Player, PlayerEvent};
use crate::strings;
use crate::ytdlp::{Resolver, StreamInfo};
use shared::youtube::{parse_youtube_url, playlist_url, watch_url, YoutubeRef};

/// Settings for the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub fake_audio: bool,
    /// Clock rate of the fake player (1.0 = perfect), to exercise drift correction.
    pub fake_rate: f64,
    pub tools: Tools,
    pub volume: f64,
    /// Run `yt-dlp -U` in the background on start.
    pub auto_update: bool,
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
    Add(String),
    SetVolume(f64),
    SetTools(Tools),
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

/// A link that was submitted but is not in the queue yet. Shown right away
/// so adding feels instant.
#[derive(Debug, Clone)]
pub struct PendingAdd {
    pub playlist: bool,
    /// Known as soon as the quick lookup finishes.
    pub title: Option<String>,
    pub started: Instant,
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
    pub player_pos_ms: Option<u64>,
    pub pending_adds: Vec<PendingAdd>,
    pub toasts: Vec<Toast>,
    pub volume: f64,
    pub player_error: Option<String>,
    pub ytdlp_version: Option<String>,
    pub ytdlp_updating: bool,
    pub fake_audio: bool,
}

impl View {
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
}

const PING_BURST: u8 = 8;
const PING_SPACING: Duration = Duration::from_millis(100);
const RESYNC_EVERY: Duration = Duration::from_secs(30);
const DRIFT_EVERY: Duration = Duration::from_secs(1);
/// Wake up this much before a scheduled unpause and spin the rest (Windows
/// timers are coarse).
const SPIN_MS: u64 = 16;
/// Max wait for the timeline to reach a pre-seeked position.
const MAX_WAIT_MS: i64 = 6000;
const MAX_TOASTS: usize = 20;
/// Drop a pending add if the server has not answered the `Add` by then.
const ADD_ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// One submitted link, from `Add` until `AddResolved` is sent.
struct AddJob {
    id: u64,
    /// Canonical URL; the server echoes it in `ResolveRequest`.
    url: String,
    playlist: bool,
    request_id: Option<u64>,
    result: Option<(Result<Vec<TrackInfo>, String>, bool)>,
    started: Instant,
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
    op_started: Option<Instant>,
    next_drift: Option<Instant>,
    next_ping: Option<Instant>,
    pings_left: u8,
    adds: Vec<AddJob>,
    next_add_id: u64,
}

async fn sleep_opt(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
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
            clock: ClockSync::default(),
            room: None,
            cur: None,
            applied_version: None,
            ready_sent: None,
            ended_sent: None,
            unpause_at: None,
            align_attempts: 0,
            load_comp_ms: 1500,
            seek_comp_ms: 300,
            op_started: None,
            next_drift: None,
            next_ping: None,
            pings_left: 0,
            adds: Vec::new(),
            next_add_id: 0,
        }
    }

    async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<Command>) {
        self.start_ytdlp_maintenance();
        loop {
            let spin_target = self.unpause_at;
            let wake_unpause = spin_target.map(|t| t - Duration::from_millis(SPIN_MS));
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
        self.adds
            .retain(|j| j.request_id.is_some() || j.started.elapsed() < ADD_ANSWER_TIMEOUT);
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
            CurPhase::Loading | CurPhase::Seeking => Phase::Buffering,
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
            Command::Add(url) => match parse_youtube_url(url.trim()) {
                None => self.toast(strings::NOT_YOUTUBE, true),
                Some(r) => {
                    let url = self.start_add_job(r, None);
                    self.send(ClientMsg::Add { url });
                }
            },
            Command::SetVolume(v) => {
                self.view.volume = v.clamp(0.0, 100.0);
                if let Some(p) = &self.player {
                    p.set_volume(self.view.volume).await;
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
                self.room = Some(room_state);
                self.reconcile().await;
            }
            ServerMsg::Members { list } => self.view.members = list,
            ServerMsg::Pong {
                client_ms,
                server_ms,
            } => {
                let before = self.clock.offset_ms();
                self.clock.add_sample(client_ms, server_ms);
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
            ServerMsg::Toast { message } => self.toast(message, false),
            ServerMsg::Error { message } => {
                // An error right after `Add` (queue full, too many pending)
                // means the oldest unanswered add will never be resolved.
                if let Some(i) = self.adds.iter().position(|j| j.request_id.is_none()) {
                    self.adds.remove(i);
                }
                self.toast(message, true)
            }
            ServerMsg::Rejected { message } => {
                self.view.conn = ConnState::Rejected(message);
            }
        }
    }

    fn ping_tick(&mut self) {
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
        }
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
        match crate::mpv::Mpv::spawn(&mpv, self.view.volume, self.player_tx.clone()).await {
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
                if !cur.reresolved {
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
                            let result = resolver.stream_info(&youtube_id).await;
                            let _ = tx.send(TaskResult::Duration { youtube_id, result });
                        });
                    }
                }
                self.send(ClientMsg::AddResolved { request_id, tracks });
            }
            Err(e) => self.toast(format!("{} {e}", strings::RESOLVE_FAILED), true),
        }
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
        let url = match result {
            Ok(info) => {
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
            Err(e) => {
                tracing::warn!("resolve failed twice: {e}");
                self.give_up_track().await;
                return;
            }
        };
        self.ensure_player().await;
        let Some(room) = self.room.clone() else {
            return;
        };
        let pb = &room.playback;
        let start = match pb.status {
            PlayStatus::Playing if self.server_now() >= pb.anchor_server_ms => {
                // Late joiner: aim where the timeline will be once loaded.
                pb.position_at(self.server_now()) + self.load_comp_ms
            }
            _ => pb.anchor_position_ms,
        };
        let duration = self.cur.as_ref().and_then(|c| c.duration_ms);
        let start = match duration {
            Some(d) => start.min(d.saturating_sub(1000)),
            None => start,
        };
        let Some(player) = &self.player else { return };
        match player.load(&url, start, duration).await {
            Ok(entry) => {
                if let Some(c) = &mut self.cur {
                    c.entry = entry;
                    c.phase = CurPhase::Loading;
                    c.paused = true;
                    c.speed = 1.0;
                }
                self.op_started = Some(Instant::now());
                tracing::info!(start, "loading track");
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
                    self.load_comp_ms = ewma(self.load_comp_ms, ms.clamp(200, 8000));
                    tracing::info!(ms, comp = self.load_comp_ms, "track loaded");
                }
            }
            CurPhase::Seeking => {
                if let Some(ms) = took {
                    self.seek_comp_ms = ewma(self.seek_comp_ms, ms.clamp(20, 5000));
                }
            }
            _ => return,
        }
        cur.phase = CurPhase::Active;
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
        let same = self.cur.as_ref().is_some_and(|c| c.track_id == track.id);
        if !same {
            self.unload().await;
            self.ended_sent = None;
            self.ready_sent = None;
            self.align_attempts = 0;
            self.cur = Some(Cur {
                track_id: track.id,
                youtube_id: track.youtube_id.clone(),
                duration_ms: track.duration_ms,
                phase: CurPhase::Resolving,
                entry: 0,
                paused: true,
                speed: 1.0,
                reresolved: false,
            });
            self.spawn_resolve(track.id, track.youtube_id.clone());
            if let Some(next) = room
                .playback
                .track_index
                .and_then(|i| room.queue.get(i + 1))
            {
                self.resolver.prefetch(&next.youtube_id);
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
        let server_at = to_server_ms(at_local, self.clock.offset_ms()) as i64;
        let reach = pb.anchor_server_ms as i64 + (pos as i64 - pb.anchor_position_ms as i64);
        let wait = reach - server_at;
        let paused = self.cur.as_ref().is_none_or(|c| c.paused);
        tracing::debug!(pos, wait, paused, "align");
        if (0..=MAX_WAIT_MS).contains(&wait) {
            if !paused {
                if wait <= shared::sync::DRIFT_IGNORE_MS {
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
        } else if wait < 0 && -wait <= shared::sync::DRIFT_SEEK_MS {
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
            let now = self.server_now();
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
        self.next_drift = Some(Instant::now() + DRIFT_EVERY);
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
        let server_at = to_server_ms(at_local, self.clock.offset_ms());
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
        match decide_drift(drift, speed) {
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
