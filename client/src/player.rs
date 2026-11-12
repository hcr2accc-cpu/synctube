//! Audio player abstraction: real mpv, or a simulated player (`--fake-audio`)
//! that lets the sync logic run without audio.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;

use crate::clock::local_ms;
use crate::mpv::Mpv;

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// A load or seek finished; the player is positioned (maybe paused).
    Positioned,
    /// The file with this entry id played to the end.
    Eof { entry: u64 },
    /// The file with this entry id could not be played.
    Failed { entry: u64, error: String },
    /// The player process died.
    Crashed,
    /// The video window was closed.
    VideoClosed,
}

pub enum Player {
    Mpv(Box<Mpv>),
    Fake(FakePlayer),
}

impl Player {
    /// Loads `url` paused at `start_ms` (plus `video` as its picture, if
    /// given). Returns the entry id used in events.
    pub async fn load(
        &self,
        url: &str,
        start_ms: u64,
        duration_ms: Option<u64>,
        title: &str,
        video: Option<&str>,
    ) -> Result<u64, String> {
        match self {
            Player::Mpv(m) => m.load(url, start_ms as f64 / 1000.0, title, video).await,
            Player::Fake(f) => Ok(f.load(start_ms, duration_ms)),
        }
    }

    /// Shows or hides the video window.
    pub async fn set_video(&self, on: bool) {
        if let Player::Mpv(m) = self {
            let (vid, window) = if on { ("1", "yes") } else { ("no", "no") };
            let _ = m.set_property("vid", json!(vid)).await;
            let _ = m.set_property("force-window", json!(window)).await;
        }
    }

    /// Adds a video track to the song that is playing (opened in the
    /// background, so playback does not stall).
    pub async fn add_video(&self, url: &str) {
        if let Player::Mpv(m) = self {
            if let Err(e) = m.command_async(json!(["video-add", url, "select"])).await {
                tracing::warn!("cannot add video: {e}");
            }
        }
    }

    /// Replaces the audio filter chain ("" removes it).
    pub async fn set_af(&self, af: &str) {
        if let Player::Mpv(m) = self {
            if let Err(e) = m.set_property("af", json!(af)).await {
                tracing::warn!("cannot set audio filter: {e}");
            }
        }
    }

    /// Changes a parameter of a running equalizer filter in place.
    pub async fn eq_command(&self, target: &str, cmd: &str, arg: &str) {
        if let Player::Mpv(m) = self {
            let r = m
                .command(json!(["af-command", crate::eq::LABEL, cmd, arg, target]))
                .await;
            if let Err(e) = r {
                tracing::debug!(target, "eq command failed: {e}");
            }
        }
    }

    pub async fn set_paused(&self, paused: bool) {
        match self {
            Player::Mpv(m) => {
                let _ = m.set_property("pause", json!(paused)).await;
            }
            Player::Fake(f) => f.set_paused(paused),
        }
    }

    pub async fn seek(&self, pos_ms: u64) {
        match self {
            Player::Mpv(m) => {
                if let Err(e) = m.seek(pos_ms as f64 / 1000.0).await {
                    tracing::warn!("mpv seek failed: {e}");
                }
            }
            Player::Fake(f) => f.seek(pos_ms),
        }
    }

    pub async fn set_speed(&self, speed: f64) {
        match self {
            Player::Mpv(m) => {
                let _ = m.set_property("speed", json!(speed)).await;
            }
            Player::Fake(f) => f.set_speed(speed),
        }
    }

    pub async fn set_volume(&self, volume: f64) {
        match self {
            Player::Mpv(m) => {
                let _ = m
                    .set_property("volume", json!(volume.clamp(0.0, 100.0)))
                    .await;
            }
            Player::Fake(f) => f.state.lock().unwrap().volume = volume,
        }
    }

    /// Fixed per-song gain in dB (loudness normalization). mpv applies it in
    /// its mixer, so it does not touch timing or the sound itself.
    pub async fn set_gain(&self, db: f64) {
        match self {
            Player::Mpv(m) => {
                if let Err(e) = m.set_property("volume-gain", json!(db)).await {
                    tracing::warn!("cannot set volume gain: {e}");
                }
            }
            Player::Fake(f) => f.state.lock().unwrap().gain_db = db,
        }
    }

    /// Current position and the local time (ms) it refers to.
    pub async fn position(&self) -> Option<(u64, u64)> {
        match self {
            Player::Mpv(m) => {
                let t0 = local_ms();
                let pos = m.get_f64("time-pos").await?;
                let t1 = local_ms();
                Some(((pos.max(0.0) * 1000.0) as u64, (t0 + t1) / 2))
            }
            Player::Fake(f) => Some((f.position_ms(), local_ms())),
        }
    }

    pub async fn stop(&self) {
        match self {
            Player::Mpv(m) => m.stop().await,
            Player::Fake(f) => f.stop(),
        }
    }

    pub async fn shutdown(&self) {
        match self {
            Player::Mpv(m) => m.quit().await,
            Player::Fake(f) => f.stop(),
        }
    }
}

#[derive(Debug)]
pub struct FakeState {
    entry: u64,
    loaded: bool,
    /// Position is unknown while "loading".
    loading_until: Option<Instant>,
    base_pos_ms: f64,
    base_at: Instant,
    paused: bool,
    speed: f64,
    duration_ms: Option<u64>,
    pub volume: f64,
    pub gain_db: f64,
}

impl FakeState {
    fn pos(&self) -> f64 {
        if self.paused || self.loading_until.is_some() {
            self.base_pos_ms
        } else {
            self.base_pos_ms + self.base_at.elapsed().as_secs_f64() * 1000.0 * self.speed
        }
    }

    fn rebase(&mut self) {
        self.base_pos_ms = self.pos();
        self.base_at = Instant::now();
    }
}

/// Simulated player. `rate` makes its clock run fast or slow to exercise
/// drift correction (1.0 = perfect).
pub struct FakePlayer {
    pub state: Arc<Mutex<FakeState>>,
    load_delay: Duration,
}

impl FakePlayer {
    pub fn new(
        events: mpsc::UnboundedSender<PlayerEvent>,
        rate: f64,
        load_delay: Duration,
    ) -> Self {
        let state = Arc::new(Mutex::new(FakeState {
            entry: 0,
            loaded: false,
            loading_until: None,
            base_pos_ms: 0.0,
            base_at: Instant::now(),
            paused: true,
            speed: 1.0,
            duration_ms: None,
            volume: 100.0,
            gain_db: 0.0,
        }));
        // Background ticker: finishes simulated loads and detects EOF.
        let st = state.clone();
        let ev = events;
        let rate = rate.clamp(0.5, 2.0);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            let mut last = Instant::now();
            loop {
                tick.tick().await;
                let now = Instant::now();
                let mut s = st.lock().unwrap();
                // Simulated clock error: advance an extra (rate - 1) of real time.
                if (rate - 1.0).abs() > 1e-9 && !s.paused && s.loading_until.is_none() && s.loaded {
                    s.base_pos_ms +=
                        now.duration_since(last).as_secs_f64() * 1000.0 * s.speed * (rate - 1.0);
                }
                last = now;
                if let Some(t) = s.loading_until {
                    if now >= t {
                        s.loading_until = None;
                        s.base_at = now;
                        let _ = ev.send(PlayerEvent::Positioned);
                    }
                }
                if s.loaded && s.loading_until.is_none() {
                    if let Some(d) = s.duration_ms {
                        if s.pos() >= d as f64 {
                            s.loaded = false;
                            s.paused = true;
                            s.base_pos_ms = d as f64;
                            let _ = ev.send(PlayerEvent::Eof { entry: s.entry });
                        }
                    }
                }
            }
        });
        Self { state, load_delay }
    }

    fn load(&self, start_ms: u64, duration_ms: Option<u64>) -> u64 {
        let mut s = self.state.lock().unwrap();
        s.entry += 1;
        s.loaded = true;
        s.paused = true;
        s.base_pos_ms = start_ms as f64;
        s.base_at = Instant::now();
        s.duration_ms = duration_ms;
        s.loading_until = Some(Instant::now() + self.load_delay);
        s.entry
    }

    fn set_paused(&self, paused: bool) {
        let mut s = self.state.lock().unwrap();
        s.rebase();
        s.paused = paused;
    }

    fn seek(&self, pos_ms: u64) {
        let mut s = self.state.lock().unwrap();
        s.base_pos_ms = pos_ms as f64;
        s.base_at = Instant::now();
        s.loading_until = Some(Instant::now() + Duration::from_millis(40));
        s.loaded = true;
    }

    fn set_speed(&self, speed: f64) {
        let mut s = self.state.lock().unwrap();
        s.rebase();
        s.speed = speed;
    }

    fn position_ms(&self) -> u64 {
        self.state.lock().unwrap().pos().max(0.0) as u64
    }

    fn stop(&self) {
        let mut s = self.state.lock().unwrap();
        s.loaded = false;
        s.paused = true;
        s.loading_until = None;
        s.base_pos_ms = 0.0;
    }
}
