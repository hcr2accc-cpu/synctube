//! One task per WebSocket connection: join handshake, rate limiting, pings,
//! forwarding commands to the room actor.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures_util::{SinkExt, StreamExt};
use shared::limits::{
    clamp_chars, MAX_NAME_CHARS, MAX_PASSWORD_CHARS, MAX_ROOM_CHARS, RATE_LIMIT_BURST,
    RATE_LIMIT_PER_SEC,
};
use shared::{ClientMsg, ServerMsg};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::room::RoomCmd;
use crate::{now_ms, strings, AppState};

/// Messages queued for the writer task.
pub(crate) enum Out {
    Text(Arc<str>),
    Close,
}

const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Token bucket rate limiter.
struct RateLimiter {
    tokens: f64,
    last: Instant,
    warned_at: Option<Instant>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            tokens: RATE_LIMIT_BURST as f64,
            last: Instant::now(),
            warned_at: None,
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * RATE_LIMIT_PER_SEC as f64).min(RATE_LIMIT_BURST as f64);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// True at most once per second, so the warning itself is not spammy.
    fn should_warn(&mut self) -> bool {
        let now = Instant::now();
        if self
            .warned_at
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1))
        {
            self.warned_at = Some(now);
            true
        } else {
            false
        }
    }
}

fn to_out(msg: &ServerMsg) -> Out {
    Out::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

pub(crate) async fn handle_socket(socket: WebSocket, app: AppState) {
    let conn = Uuid::new_v4();
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(256);
    let ping_every = app.config().ws_ping_interval;
    let idle_timeout = app.config().idle_timeout;

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(ping_every);
        ping.tick().await;
        loop {
            tokio::select! {
                out = out_rx.recv() => match out {
                    Some(Out::Text(t)) => {
                        if sink.send(Message::Text(Utf8Bytes::from(t.as_ref()))).await.is_err() {
                            break;
                        }
                    }
                    Some(Out::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut limiter = RateLimiter::new();
    let mut room: Option<crate::RoomHandle> = None;
    let join_deadline = Instant::now() + JOIN_TIMEOUT;

    loop {
        let wait = if room.is_none() {
            join_deadline.saturating_duration_since(Instant::now())
        } else {
            idle_timeout
        };
        let frame = match tokio::time::timeout(wait, stream.next()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(Some(Err(e))) => {
                tracing::debug!(%conn, error = %e, "websocket error");
                break;
            }
            Ok(None) => break,
            Err(_) => {
                tracing::debug!(%conn, "connection idle or join timeout");
                break;
            }
        };
        let text = match frame {
            Message::Text(t) => t,
            Message::Close(_) => break,
            // Pings are answered automatically; pongs just keep the connection alive.
            _ => continue,
        };
        if !limiter.allow() {
            if limiter.should_warn() {
                let _ = out_tx.try_send(to_out(&ServerMsg::Error {
                    message: strings::RATE_LIMITED.to_string(),
                }));
            }
            continue;
        }
        let msg: ClientMsg = match serde_json::from_str(text.as_str()) {
            Ok(m) => m,
            Err(_) => {
                let _ = out_tx.try_send(to_out(&ServerMsg::Error {
                    message: strings::BAD_MESSAGE.to_string(),
                }));
                continue;
            }
        };
        match msg {
            ClientMsg::Ping { client_ms } => {
                let _ = out_tx.try_send(to_out(&ServerMsg::Pong {
                    client_ms,
                    server_ms: now_ms(),
                }));
            }
            ClientMsg::Leave => break,
            ClientMsg::Join {
                room: room_name,
                password,
                name,
            } if room.is_none() => match join(&app, conn, room_name, password, name, &out_tx).await
            {
                Ok(handle) => room = Some(handle),
                Err(reason) => {
                    let _ = out_tx.try_send(to_out(&ServerMsg::Rejected { message: reason }));
                    let _ = out_tx.try_send(Out::Close);
                    drop(out_tx);
                    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
                    return;
                }
            },
            other => match &room {
                Some(h) => {
                    if h.tx.send(RoomCmd::Msg { conn, msg: other }).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = out_tx.try_send(to_out(&ServerMsg::Error {
                        message: strings::JOIN_FIRST.to_string(),
                    }));
                }
            },
        }
    }

    if let Some(h) = room {
        let _ = h.tx.send(RoomCmd::Leave { conn }).await;
    }
    writer.abort();
}

async fn join(
    app: &AppState,
    conn: Uuid,
    room: String,
    password: Option<String>,
    name: String,
    out: &mpsc::Sender<Out>,
) -> Result<crate::RoomHandle, String> {
    let room = clamp_chars(&room, MAX_ROOM_CHARS).to_lowercase();
    if room.is_empty() || room.chars().any(char::is_control) {
        return Err(strings::BAD_ROOM.to_string());
    }
    let name = clamp_chars(&name, MAX_NAME_CHARS);
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(strings::BAD_NAME.to_string());
    }
    let password = password.map(|p| p.chars().take(MAX_PASSWORD_CHARS).collect::<String>());
    // The room actor may be shutting down right when we fetch its handle;
    // in that case a fresh one is created on retry.
    for _ in 0..3 {
        let handle = app.room_handle(&room);
        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = RoomCmd::Join {
            conn,
            name: name.clone(),
            password: password.clone(),
            out: out.clone(),
            reply: reply_tx,
        };
        if handle.tx.send(cmd).await.is_err() {
            continue;
        }
        match reply_rx.await {
            Ok(Ok(())) => return Ok(handle),
            Ok(Err(reason)) => return Err(reason),
            Err(_) => continue,
        }
    }
    Err(strings::SERVER_BUSY.to_string())
}
