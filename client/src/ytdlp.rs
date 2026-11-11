//! yt-dlp wrapper: stream URL resolution (cached, deduplicated, prefetchable)
//! and metadata resolution for `Add`. Everything runs as async child processes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use shared::youtube::{parse_youtube_url, watch_url, YoutubeRef};
use shared::TrackInfo;
use tokio::sync::oneshot;

use crate::strings;

/// Resolved stream URLs are reused for this long (they expire after ~6 h).
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const STREAM_TIMEOUT: Duration = Duration::from_secs(45);
const META_TIMEOUT: Duration = Duration::from_secs(120);
const OEMBED_TIMEOUT: Duration = Duration::from_secs(5);
/// Duration used for synthetic tracks in fake mode without yt-dlp.
pub const FAKE_DURATION_MS: u64 = 30_000;

/// A resolved audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub url: String,
    /// Known when yt-dlp reports it (used to fill in quick-added tracks).
    pub duration_ms: Option<u64>,
}

type Waiters = Vec<oneshot::Sender<Result<StreamInfo, String>>>;

#[derive(Default)]
struct Inner {
    cache: HashMap<String, (StreamInfo, Instant)>,
    inflight: HashMap<String, Waiters>,
}

#[derive(Clone)]
pub struct Resolver {
    yt_dlp: Option<PathBuf>,
    fake: bool,
    inner: Arc<Mutex<Inner>>,
}

impl Resolver {
    pub fn new(yt_dlp: Option<PathBuf>, fake: bool) -> Self {
        Self {
            yt_dlp,
            fake,
            inner: Arc::default(),
        }
    }

    pub fn is_fake(&self) -> bool {
        self.fake
    }

    pub fn set_path(&mut self, yt_dlp: Option<PathBuf>) {
        self.yt_dlp = yt_dlp;
    }

    /// Drops a cached URL (e.g. after mpv failed to play it).
    pub fn invalidate(&self, youtube_id: &str) {
        self.inner.lock().unwrap().cache.remove(youtube_id);
    }

    /// Resolves in the background so the URL is cached when needed.
    pub fn prefetch(&self, youtube_id: &str) {
        let me = self.clone();
        let id = youtube_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = me.stream_info(&id).await {
                tracing::debug!(id, "prefetch failed: {e}");
            }
        });
    }

    /// Direct audio stream URL for a video. Retries once on failure.
    pub async fn stream_url(&self, youtube_id: &str) -> Result<String, String> {
        self.stream_info(youtube_id).await.map(|s| s.url)
    }

    /// Stream URL plus duration. Cached for an hour and deduplicated, so a
    /// background duration lookup also warms the cache for playback.
    pub async fn stream_info(&self, youtube_id: &str) -> Result<StreamInfo, String> {
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            if let Some((info, at)) = inner.cache.get(youtube_id) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(info.clone());
                }
            }
            let (tx, rx) = oneshot::channel();
            match inner.inflight.get_mut(youtube_id) {
                Some(waiters) => {
                    waiters.push(tx);
                    Some(rx)
                }
                None => {
                    inner.inflight.insert(youtube_id.to_string(), vec![tx]);
                    let me = self.clone();
                    let id = youtube_id.to_string();
                    tokio::spawn(async move { me.resolve_and_notify(id).await });
                    Some(rx)
                }
            }
        };
        match rx {
            Some(rx) => rx
                .await
                .unwrap_or_else(|_| Err(strings::RESOLVE_FAILED.into())),
            None => Err(strings::RESOLVE_FAILED.into()),
        }
    }

    async fn resolve_and_notify(&self, id: String) {
        let started = Instant::now();
        let mut result = self.resolve_stream_once(&id).await;
        if let Err(e) = &result {
            tracing::warn!(id, "stream resolve failed, retrying: {e}");
            result = self.resolve_stream_once(&id).await;
        }
        match &result {
            Ok(_) => tracing::info!(
                id,
                ms = started.elapsed().as_millis() as u64,
                "stream resolved"
            ),
            Err(e) => tracing::warn!(id, "stream resolve failed: {e}"),
        }
        let waiters = {
            let mut inner = self.inner.lock().unwrap();
            if let Ok(info) = &result {
                inner
                    .cache
                    .insert(id.clone(), (info.clone(), Instant::now()));
                inner.cache.retain(|_, (_, at)| at.elapsed() < CACHE_TTL);
            }
            inner.inflight.remove(&id).unwrap_or_default()
        };
        for w in waiters {
            let _ = w.send(result.clone());
        }
    }

    async fn resolve_stream_once(&self, id: &str) -> Result<StreamInfo, String> {
        if self.fake {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(StreamInfo {
                url: format!("fake://{id}"),
                duration_ms: None,
            });
        }
        // `-J` with a single selected format puts that format's URL at the top
        // level, so one run yields the stream URL and the duration.
        let out = self
            .run(
                &[
                    "-J",
                    "-f",
                    "bestaudio[ext=webm]/bestaudio",
                    "--no-playlist",
                    "--no-warnings",
                    &watch_url(id),
                ],
                STREAM_TIMEOUT,
            )
            .await?;
        let v: Value =
            serde_json::from_str(&out).map_err(|_| strings::RESOLVE_FAILED.to_string())?;
        parse_stream_info(&v).ok_or_else(|| strings::RESOLVE_FAILED.to_string())
    }

    /// Fast title lookup for a single video through YouTube's oEmbed endpoint
    /// (a few hundred ms instead of several seconds for yt-dlp). No duration.
    pub async fn quick_video(&self, id: &str) -> Result<TrackInfo, String> {
        if self.fake {
            return Err("fake".into());
        }
        let id = id.to_string();
        tokio::task::spawn_blocking(move || oembed_title(&id).map(|title| (id, title)))
            .await
            .map_err(|e| e.to_string())?
            .map(|(youtube_id, title)| TrackInfo {
                youtube_id,
                title,
                duration_ms: None,
            })
    }

    /// Resolves the tracks behind a (canonical) YouTube URL.
    pub async fn metadata(&self, url: &str) -> Result<Vec<TrackInfo>, String> {
        let parsed = parse_youtube_url(url).ok_or_else(|| strings::NOT_YOUTUBE.to_string())?;
        if self.yt_dlp.is_none() && self.fake {
            return Ok(fake_metadata(&parsed));
        }
        let json = match &parsed {
            YoutubeRef::Playlist(_) => {
                self.run(
                    &["--flat-playlist", "-J", "--no-warnings", url],
                    META_TIMEOUT,
                )
                .await?
            }
            YoutubeRef::Video(_) => {
                // Same format selection as playback, so the result also warms
                // the stream cache.
                self.run(
                    &[
                        "-J",
                        "-f",
                        "bestaudio[ext=webm]/bestaudio",
                        "--no-playlist",
                        "--no-warnings",
                        url,
                    ],
                    META_TIMEOUT,
                )
                .await?
            }
        };
        let v: Value =
            serde_json::from_str(&json).map_err(|_| strings::RESOLVE_FAILED.to_string())?;
        if let (YoutubeRef::Video(id), Some(info)) = (&parsed, parse_stream_info(&v)) {
            self.inner
                .lock()
                .unwrap()
                .cache
                .insert(id.clone(), (info, Instant::now()));
        }
        let tracks = parse_metadata(&v);
        if tracks.is_empty() {
            return Err(strings::NOTHING_FOUND.into());
        }
        Ok(tracks)
    }

    /// `yt-dlp --version`.
    pub async fn version(&self) -> Option<String> {
        self.run(&["--version"], Duration::from_secs(30))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// `yt-dlp -U` (self-update). Returns the tool output.
    pub async fn self_update(&self) -> Result<String, String> {
        self.run(&["-U"], Duration::from_secs(180)).await
    }

    async fn run(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let path = self
            .yt_dlp
            .as_ref()
            .ok_or_else(|| strings::YTDLP_MISSING.to_string())?;
        let mut cmd = crate::proc::command(path);
        cmd.args(args);
        let child = cmd.spawn().map_err(|e| format!("yt-dlp: {e}"))?;
        crate::proc::bind_to_job(&child);
        let out = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| strings::RESOLVE_TIMEOUT.to_string())?
            .map_err(|e| format!("yt-dlp: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err
                .lines()
                .rev()
                .find(|l| l.contains("ERROR"))
                .unwrap_or_else(|| err.lines().last().unwrap_or(""))
                .trim()
                .to_string();
            Err(if line.is_empty() {
                strings::RESOLVE_FAILED.to_string()
            } else {
                line
            })
        }
    }
}

fn oembed_title(id: &str) -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(OEMBED_TIMEOUT))
        .user_agent("SyncTube")
        .build()
        .new_agent();
    let url = format!(
        "https://www.youtube.com/oembed?format=json&url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3D{id}"
    );
    let body = agent
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    v.get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .ok_or_else(|| "no title".into())
}

/// Parses `yt-dlp -J -f <single format>` output.
pub fn parse_stream_info(v: &Value) -> Option<StreamInfo> {
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("http"))?;
    Some(StreamInfo {
        url: url.to_string(),
        duration_ms: duration_ms(v),
    })
}

fn duration_ms(v: &Value) -> Option<u64> {
    v.get("duration")
        .and_then(Value::as_f64)
        .filter(|d| *d > 0.0)
        .map(|d| (d * 1000.0) as u64)
}

/// Parses `yt-dlp -J` (video) or `--flat-playlist -J` (playlist) output.
pub fn parse_metadata(v: &Value) -> Vec<TrackInfo> {
    let one = |e: &Value| -> Option<TrackInfo> {
        let id = e.get("id")?.as_str()?;
        if !shared::youtube::is_valid_video_id(id) {
            return None;
        }
        let title = e
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        // Deleted/private playlist entries have placeholder titles.
        if title == "[Deleted video]" || title == "[Private video]" {
            return None;
        }
        Some(TrackInfo {
            youtube_id: id.to_string(),
            title,
            duration_ms: duration_ms(e),
        })
    };
    match v.get("entries").and_then(Value::as_array) {
        Some(entries) => entries
            .iter()
            .filter_map(one)
            .take(shared::limits::MAX_QUEUE)
            .collect(),
        None => one(v).into_iter().collect(),
    }
}

fn fake_metadata(r: &YoutubeRef) -> Vec<TrackInfo> {
    match r {
        YoutubeRef::Video(id) => vec![TrackInfo {
            youtube_id: id.clone(),
            title: format!("Test pesma {id}"),
            duration_ms: Some(FAKE_DURATION_MS),
        }],
        YoutubeRef::Playlist(_) => (0..3)
            .map(|i| TrackInfo {
                youtube_id: format!("fakeplayli{i}"),
                title: format!("Test pesma {}", i + 1),
                duration_ms: Some(FAKE_DURATION_MS),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_video_json() {
        let v: Value = serde_json::from_str(
            r#"{"id":"dQw4w9WgXcQ","title":"Rick","duration":213.0,"formats":[]}"#,
        )
        .unwrap();
        let t = parse_metadata(&v);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].duration_ms, Some(213_000));
    }

    #[test]
    fn parses_flat_playlist_json() {
        let v: Value = serde_json::from_str(
            r#"{"_type":"playlist","entries":[
                {"id":"aaaaaaaaaaa","title":"A","duration":10},
                {"id":"bbbbbbbbbbb","title":"[Deleted video]","duration":null},
                {"id":"bad","title":"x"},
                {"id":"ccccccccccc","title":"C","duration":null}]}"#,
        )
        .unwrap();
        let t = parse_metadata(&v);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].duration_ms, Some(10_000));
        assert_eq!(t[1].duration_ms, None);
    }

    #[test]
    fn parses_stream_info_json() {
        let v: Value = serde_json::from_str(
            r#"{"id":"dQw4w9WgXcQ","duration":213,"format_id":"251","url":"https://rr1.googlevideo.com/x"}"#,
        )
        .unwrap();
        let s = parse_stream_info(&v).unwrap();
        assert_eq!(s.url, "https://rr1.googlevideo.com/x");
        assert_eq!(s.duration_ms, Some(213_000));
        let v: Value = serde_json::from_str(r#"{"id":"dQw4w9WgXcQ"}"#).unwrap();
        assert!(parse_stream_info(&v).is_none());
    }
}
