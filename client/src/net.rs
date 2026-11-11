//! WebSocket connection with auto-reconnect and Render cold-start handling.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use shared::{ClientMsg, ServerMsg};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Backoff between reconnect attempts.
const BACKOFF: [u64; 4] = [1, 2, 5, 10];
/// Before the first successful connection we keep trying this long
/// (Render free tier can take ~60 s to wake up).
const COLD_START_LIMIT: Duration = Duration::from_secs(90);
/// After this long without a connection we tell the user the server is waking up.
const WAKING_AFTER: Duration = Duration::from_secs(3);
/// One connection attempt may take this long (Render holds the request while booting).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(70);
/// No message (incl. server pings) for this long means the connection is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub enum NetEvent {
    /// Trying to connect. `waking` = probably a cold start.
    Connecting {
        waking: bool,
    },
    /// Connected and `Join` sent.
    Connected,
    Msg(ServerMsg),
    /// Connection lost; retrying after `retry_in`.
    Disconnected {
        retry_in: Duration,
    },
    /// The server refused the join; no retry.
    Rejected(String),
    /// Could not connect at all within the cold start limit; no retry.
    Failed(String),
}

pub struct NetHandle {
    out: mpsc::UnboundedSender<ClientMsg>,
    task: tokio::task::JoinHandle<()>,
}

impl NetHandle {
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.out.send(msg);
    }

    /// Sends `Leave` and closes the connection.
    pub async fn close(self) {
        let _ = self.out.send(ClientMsg::Leave);
        drop(self.out);
        let _ = tokio::time::timeout(Duration::from_millis(500), self.task).await;
    }
}

pub fn spawn(url: String, join: ClientMsg, events: mpsc::UnboundedSender<NetEvent>) -> NetHandle {
    let (out, out_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(url, join, out_rx, events));
    NetHandle { out, task }
}

enum SessionEnd {
    /// Connection dropped; reconnect.
    Lost,
    /// Server rejected us, or the user left.
    Stop,
}

async fn run(
    url: String,
    join: ClientMsg,
    mut out_rx: mpsc::UnboundedReceiver<ClientMsg>,
    events: mpsc::UnboundedSender<NetEvent>,
) {
    let started = Instant::now();
    let mut ever_connected = false;
    let mut attempt = 0usize;
    let mut last_error = String::new();
    loop {
        let waking = !ever_connected && started.elapsed() >= WAKING_AFTER;
        let _ = events.send(NetEvent::Connecting { waking });
        let connect = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(url.as_str()),
        );
        tokio::pin!(connect);
        let mut waking_sent = waking;
        let result = loop {
            tokio::select! {
                r = &mut connect => break r,
                _ = tokio::time::sleep(WAKING_AFTER), if !ever_connected && !waking_sent => {
                    waking_sent = true;
                    let _ = events.send(NetEvent::Connecting { waking: true });
                }
                m = out_rx.recv() => {
                    if m.is_none() || matches!(m, Some(ClientMsg::Leave)) {
                        return;
                    }
                }
            }
        };
        match result {
            Ok(Ok((ws, _))) => {
                ever_connected = true;
                attempt = 0;
                match session(ws, &join, &mut out_rx, &events).await {
                    SessionEnd::Stop => return,
                    SessionEnd::Lost => {}
                }
            }
            Ok(Err(e)) => {
                last_error = e.to_string();
                tracing::info!("connect failed: {last_error}");
            }
            Err(_) => {
                last_error = "timeout".into();
                tracing::info!("connect timed out");
            }
        }
        if !ever_connected && started.elapsed() >= COLD_START_LIMIT {
            let _ = events.send(NetEvent::Failed(last_error));
            return;
        }
        let delay = Duration::from_secs(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
        attempt += 1;
        let _ = events.send(NetEvent::Disconnected { retry_in: delay });
        // Commands issued while offline are dropped; the fresh state after
        // rejoining is authoritative.
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                m = out_rx.recv() => {
                    if m.is_none() || matches!(m, Some(ClientMsg::Leave)) {
                        return;
                    }
                }
            }
        }
    }
}

async fn session<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    join: &ClientMsg,
    out_rx: &mut mpsc::UnboundedReceiver<ClientMsg>,
    events: &mpsc::UnboundedSender<NetEvent>,
) -> SessionEnd
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let join_text = serde_json::to_string(join).unwrap_or_default();
    if sink.send(Message::text(join_text)).await.is_err() {
        return SessionEnd::Lost;
    }
    let _ = events.send(NetEvent::Connected);
    loop {
        tokio::select! {
            frame = tokio::time::timeout(READ_TIMEOUT, stream.next()) => {
                let frame = match frame {
                    Ok(Some(Ok(f))) => f,
                    Ok(Some(Err(e))) => {
                        tracing::info!("websocket error: {e}");
                        return SessionEnd::Lost;
                    }
                    Ok(None) => return SessionEnd::Lost,
                    Err(_) => {
                        tracing::info!("websocket read timeout");
                        return SessionEnd::Lost;
                    }
                };
                match frame {
                    Message::Text(t) => match serde_json::from_str::<ServerMsg>(t.as_str()) {
                        Ok(ServerMsg::Rejected { message }) => {
                            let _ = events.send(NetEvent::Rejected(message));
                            return SessionEnd::Stop;
                        }
                        Ok(msg) => {
                            let _ = events.send(NetEvent::Msg(msg));
                        }
                        Err(e) => tracing::warn!("bad server message: {e}"),
                    },
                    Message::Close(_) => return SessionEnd::Lost,
                    // Ping replies are queued by tungstenite; flush them out.
                    Message::Ping(_) if sink.flush().await.is_err() => return SessionEnd::Lost,
                    _ => {}
                }
            }
            m = out_rx.recv() => {
                let Some(m) = m else {
                    let _ = sink.send(Message::Close(None)).await;
                    return SessionEnd::Stop;
                };
                let leave = matches!(m, ClientMsg::Leave);
                let text = serde_json::to_string(&m).unwrap_or_default();
                if sink.send(Message::text(text)).await.is_err() {
                    return if leave { SessionEnd::Stop } else { SessionEnd::Lost };
                }
                if leave {
                    let _ = sink.send(Message::Close(None)).await;
                    return SessionEnd::Stop;
                }
            }
        }
    }
}
