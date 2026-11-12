//! mpv child process controlled over its JSON IPC (a named pipe on Windows,
//! a Unix domain socket elsewhere).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::sync::{mpsc, oneshot};

use crate::player::PlayerEvent;

#[cfg(windows)]
type Pipe = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(unix)]
type Pipe = tokio::net::UnixStream;

const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// Sent by the video window's close button / Q (see [`INPUT_CONF`]).
pub const VIDEO_CLOSED_MSG: &str = "synctube-video-closed";
/// Key bindings of the video window. Nothing that could pause or seek
/// locally (that would break sync): only fullscreen and close.
const INPUT_CONF: &str = "\
CLOSE_WIN script-message synctube-video-closed
q script-message synctube-video-closed
f cycle fullscreen
MBTN_LEFT_DBL cycle fullscreen
ESC set fullscreen no
";

/// Startup settings of the player.
pub struct SpawnOpts {
    pub volume: f64,
    /// Show the video of each song in a window.
    pub video: bool,
    /// Audio filter (equalizer), "" for none.
    pub af: String,
    /// Native window inside the app to draw the video into.
    pub wid: Option<i64>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Mpv {
    _child: tokio::process::Child,
    writer: tokio::sync::Mutex<WriteHalf<Pipe>>,
    pending: Pending,
    next_id: AtomicU64,
}

impl Mpv {
    /// Starts mpv and connects to its IPC pipe.
    pub async fn spawn(
        mpv: &Path,
        opts: &SpawnOpts,
        events: mpsc::UnboundedSender<PlayerEvent>,
    ) -> Result<Self, String> {
        let pipe_name = ipc_name();
        let mut cmd = crate::proc::command(mpv);
        cmd.args([
            "--idle=yes",
            "--no-terminal",
            &format!("--input-ipc-server={pipe_name}"),
            "--cache=yes",
            "--demuxer-max-bytes=32MiB",
            &format!("--volume={}", opts.volume.clamp(0.0, 100.0)),
            "--ytdl=no",
            "--audio-display=no",
            "--input-default-bindings=no",
            "--load-scripts=no",
            "--osc=no",
            "--hr-seek=yes",
            // Video (when on) comes as an extra file; it is track 1 because
            // the audio stream itself has no video.
            video_args(opts.video).0,
            video_args(opts.video).1,
            "--hwdec=auto-safe",
            "--keep-open=no",
        ]);
        match opts.wid {
            // Inside the app window; mouse and keys stay with the app.
            Some(wid) => {
                cmd.args([
                    &format!("--wid={wid}"),
                    "--input-cursor=no",
                    "--input-vo-keyboard=no",
                    "--cursor-autohide=no",
                ]);
            }
            None => {
                cmd.args([
                    "--title=SyncTube",
                    "--geometry=50%:50%",
                    "--autofit=960x540",
                    "--autofit-larger=90%x90%",
                ]);
            }
        }
        if let Some(conf) = input_conf() {
            cmd.arg(format!("--input-conf={}", conf.display()));
        }
        if !opts.af.is_empty() {
            cmd.arg(format!("--af={}", opts.af));
        }
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd.spawn().map_err(|e| format!("mpv: {e}"))?;
        crate::proc::bind_to_job(&child);

        let pipe = connect_pipe(&pipe_name).await?;
        let (reader, writer) = tokio::io::split(pipe);
        let pending: Pending = Arc::default();
        tokio::spawn(read_loop(reader, pending.clone(), events));

        let mpv = Self {
            _child: child,
            writer: tokio::sync::Mutex::new(writer),
            pending,
            next_id: AtomicU64::new(1),
        };
        // Only cheap, event-driven observations; time-pos is queried on demand
        // (once per second while playing) to keep CPU usage near zero.
        mpv.command(json!(["observe_property", 1, "pause"])).await?;
        mpv.command(json!(["observe_property", 2, "eof-reached"]))
            .await?;
        Ok(mpv)
    }

    /// Sends a command and waits for its reply.
    pub async fn command(&self, args: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut line = serde_json::to_vec(&json!({ "command": args, "request_id": id }))
            .map_err(|e| e.to_string())?;
        line.push(b'\n');
        {
            let mut w = self.writer.lock().await;
            if let Err(e) = w.write_all(&line).await {
                self.pending.lock().unwrap().remove(&id);
                return Err(format!("mpv ipc: {e}"));
            }
        }
        match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("mpv ipc closed".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err("mpv ipc timeout".into())
            }
        }
    }

    pub async fn set_property(&self, name: &str, value: Value) -> Result<(), String> {
        self.command(json!(["set_property", name, value]))
            .await
            .map(|_| ())
    }

    /// Sends a command that mpv runs in the background (e.g. opening a
    /// stream); does not wait for it.
    pub async fn command_async(&self, args: Value) -> Result<(), String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut line =
            serde_json::to_vec(&json!({ "command": args, "request_id": id, "async": true }))
                .map_err(|e| e.to_string())?;
        line.push(b'\n');
        let mut w = self.writer.lock().await;
        w.write_all(&line)
            .await
            .map_err(|e| format!("mpv ipc: {e}"))
    }

    pub async fn get_f64(&self, name: &str) -> Option<f64> {
        self.command(json!(["get_property", name]))
            .await
            .ok()
            .and_then(|v| v.as_f64())
    }

    /// Loads `url` paused at `start_s`, with `video` as an extra video
    /// track. Returns mpv's playlist entry id.
    pub async fn load(
        &self,
        url: &str,
        start_s: f64,
        title: &str,
        video: Option<&str>,
    ) -> Result<u64, String> {
        self.set_property("pause", json!(true)).await?;
        self.set_property("start", json!(format!("{start_s:.3}")))
            .await?;
        let extra: Vec<&str> = video.into_iter().collect();
        if let Err(e) = self.set_property("external-files", json!(extra)).await {
            tracing::warn!("cannot set the video track: {e}");
        }
        let _ = self.set_property("force-media-title", json!(title)).await;
        let r = self.command(json!(["loadfile", url, "replace"])).await?;
        Ok(r.get("playlist_entry_id")
            .and_then(Value::as_u64)
            .unwrap_or(0))
    }

    pub async fn seek(&self, pos_s: f64) -> Result<(), String> {
        self.command(json!(["seek", pos_s, "absolute+exact"]))
            .await
            .map(|_| ())
    }

    pub async fn stop(&self) {
        let _ = self.command(json!(["stop"])).await;
    }

    pub async fn quit(&self) {
        let _ = self.command(json!(["quit"])).await;
    }
}

/// `--vid` / `--force-window` for video on or off.
pub fn video_args(on: bool) -> (&'static str, &'static str) {
    if on {
        ("--vid=1", "--force-window=yes")
    } else {
        ("--vid=no", "--force-window=no")
    }
}

/// Writes the video window's key bindings next to the config.
fn input_conf() -> Option<std::path::PathBuf> {
    let path = crate::paths::app_dir().join("mpv-input.conf");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(INPUT_CONF) {
        let _ = std::fs::create_dir_all(crate::paths::app_dir());
        std::fs::write(&path, INPUT_CONF).ok()?;
    }
    Some(path)
}

#[cfg(windows)]
fn ipc_name() -> String {
    format!(r"\\.\pipe\synctube-{}", std::process::id())
}

/// A socket path in the temp dir; a stale one from a crashed mpv is removed.
#[cfg(unix)]
fn ipc_name() -> String {
    let path = std::env::temp_dir().join(format!("synctube-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path.to_string_lossy().into_owned()
}

#[cfg(unix)]
async fn connect_pipe(name: &str) -> Result<Pipe, String> {
    for _ in 0..100 {
        match tokio::net::UnixStream::connect(name).await {
            Ok(p) => return Ok(p),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("mpv socket: {e}")),
        }
    }
    Err("mpv did not open its IPC socket".into())
}

#[cfg(windows)]
async fn connect_pipe(name: &str) -> Result<Pipe, String> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    for _ in 0..100 {
        match ClientOptions::new().open(name) {
            Ok(p) => return Ok(p),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(ERROR_PIPE_BUSY) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(format!("mpv pipe: {e}")),
        }
    }
    Err("mpv did not open its IPC pipe".into())
}

async fn read_loop(
    reader: tokio::io::ReadHalf<Pipe>,
    pending: Pending,
    events: mpsc::UnboundedSender<PlayerEvent>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(id) = v.get("request_id").and_then(Value::as_u64) {
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let ok = v.get("error").and_then(Value::as_str) == Some("success");
                let _ = tx.send(if ok {
                    Ok(v.get("data").cloned().unwrap_or(Value::Null))
                } else {
                    Err(v
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_string())
                });
            }
            continue;
        }
        let entry = v
            .get("playlist_entry_id")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        match v.get("event").and_then(Value::as_str) {
            Some("playback-restart") => {
                let _ = events.send(PlayerEvent::Positioned);
            }
            Some("client-message") => {
                let first = v
                    .get("args")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(Value::as_str);
                if first == Some(VIDEO_CLOSED_MSG) {
                    let _ = events.send(PlayerEvent::VideoClosed);
                }
            }
            Some("end-file") => match v.get("reason").and_then(Value::as_str) {
                Some("eof") => {
                    let _ = events.send(PlayerEvent::Eof { entry });
                }
                Some("error") => {
                    let err = v
                        .get("file_error")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_string();
                    let _ = events.send(PlayerEvent::Failed { entry, error: err });
                }
                _ => {}
            },
            _ => {}
        }
    }
    let _ = events.send(PlayerEvent::Crashed);
}
