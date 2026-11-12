//! SyncTube server: authoritative room state, never touches audio.

mod conn;
mod room;
pub mod strings;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::room::RoomCmd;

/// Tunables. Defaults follow the spec; tests use shorter timeouts.
#[derive(Debug, Clone)]
pub struct Config {
    /// Max time to wait for all clients to report `Ready` before starting.
    pub barrier_timeout: Duration,
    /// Max wait for `Ready` after a seek while playing.
    pub seek_barrier_timeout: Duration,
    /// Scheduled start lead when a track (or seek) starts after the barrier.
    pub start_lead_ms: u64,
    /// Scheduled start lead when resuming from pause.
    pub resume_lead_ms: u64,
    /// Grace after the track duration before the server auto-advances.
    pub end_grace_ms: u64,
    /// Empty rooms are deleted after this long.
    pub empty_room_ttl: Duration,
    /// WebSocket ping interval.
    pub ws_ping_interval: Duration,
    /// Connections that send nothing (not even pongs) for this long are closed.
    pub idle_timeout: Duration,
    /// Max members per room.
    pub max_members: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            barrier_timeout: Duration::from_secs(6),
            seek_barrier_timeout: Duration::from_secs(3),
            start_lead_ms: 400,
            resume_lead_ms: 250,
            end_grace_ms: 2000,
            empty_room_ttl: Duration::from_secs(30 * 60),
            ws_ping_interval: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(70),
            max_members: 50,
        }
    }
}

/// Monotonic server time in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    static BASE: OnceLock<(Instant, u64)> = OnceLock::new();
    let (inst, epoch) = BASE.get_or_init(|| {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        (Instant::now(), epoch)
    });
    epoch + inst.elapsed().as_millis() as u64
}

#[derive(Clone)]
pub(crate) struct RoomHandle {
    pub tx: mpsc::Sender<RoomCmd>,
    pub generation: u64,
}

pub(crate) struct Inner {
    pub config: Config,
    pub rooms: Mutex<HashMap<String, RoomHandle>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct AppState(pub Arc<Inner>);

impl AppState {
    /// Returns the handle of `room`, spawning the room actor if needed.
    pub fn room_handle(&self, room: &str) -> RoomHandle {
        let mut rooms = self.0.rooms.lock().expect("rooms lock");
        if let Some(h) = rooms.get(room) {
            if !h.tx.is_closed() {
                return h.clone();
            }
        }
        let generation = self.0.next_generation.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(512);
        let handle = RoomHandle { tx, generation };
        rooms.insert(room.to_string(), handle.clone());
        tokio::spawn(room::run_room(
            room.to_string(),
            generation,
            rx,
            self.clone(),
        ));
        tracing::info!(room, "room created");
        handle
    }

    /// Removes the registry entry if it still belongs to `generation`.
    pub fn remove_room(&self, room: &str, generation: u64) {
        let mut rooms = self.0.rooms.lock().expect("rooms lock");
        if rooms.get(room).map(|h| h.generation) == Some(generation) {
            rooms.remove(room);
            tracing::info!(room, "room deleted");
        }
    }

    pub fn config(&self) -> &Config {
        &self.0.config
    }
}

/// Builds the HTTP router (`/health` and `/ws`).
pub fn app(config: Config) -> Router {
    let state = AppState(Arc::new(Inner {
        config,
        rooms: Mutex::new(HashMap::new()),
        next_generation: AtomicU64::new(1),
    }));
    Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn root() -> &'static str {
    "SyncTube server. WebSocket endpoint: /ws"
}

async fn health() -> &'static str {
    "ok"
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.max_message_size(shared::limits::MAX_MESSAGE_BYTES)
        .max_frame_size(shared::limits::MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| conn::handle_socket(socket, state))
}

/// Serves on an already bound listener until the process ends.
pub async fn serve(listener: TcpListener, config: Config) -> std::io::Result<()> {
    use axum::serve::ListenerExt;
    // Small messages (pongs, state updates) go out at once instead of
    // waiting for Nagle's algorithm: lower ping and tighter sync.
    let listener = listener.tap_io(|tcp| {
        if let Err(e) = tcp.set_nodelay(true) {
            tracing::debug!("cannot set TCP_NODELAY: {e}");
        }
    });
    axum::serve(listener, app(config)).await
}
