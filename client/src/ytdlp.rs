//! yt-dlp wrapper: stream URL resolution (cached, deduplicated, prefetchable)
//! and metadata resolution for `Add`. Everything runs as async child processes.

use std::collections::{HashMap, HashSet};
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
const SEARCH_TIMEOUT: Duration = Duration::from_secs(40);
/// How many results a YouTube search returns.
pub const SEARCH_RESULTS: usize = 20;
/// Duration used for synthetic tracks in fake mode without yt-dlp.
pub const FAKE_DURATION_MS: u64 = 30_000;

/// yt-dlp format selection: the audio stream, plus a separate video stream
/// (up to 1080p, H.264 preferred for hardware decoding) for video mode.
/// One run yields both, so video mode costs no extra resolve time; nothing
/// is merged (mpv plays the two streams together).
const FORMAT: &str = "bv*[height<=1080][vcodec^=avc1]+ba[ext=webm]/bv*[height<=1080]+ba[ext=webm]/bv*[height<=1080]+ba/ba[ext=webm]/ba";

/// A resolved audio stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub url: String,
    /// Known when yt-dlp reports it (used to fill in quick-added tracks).
    pub duration_ms: Option<u64>,
    /// The matching video-only stream, for video mode.
    pub video_url: Option<String>,
}

/// One YouTube search result: a video, or a playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Video id, or playlist id when `playlist` is set.
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration_ms: Option<u64>,
    pub playlist: bool,
    /// Video whose thumbnail stands for the hit.
    pub thumb_id: Option<String>,
}

impl SearchHit {
    /// The link that adds this hit to the queue.
    pub fn url(&self) -> String {
        if self.playlist {
            shared::youtube::playlist_url(&self.youtube_id)
        } else {
            watch_url(&self.youtube_id)
        }
    }
}

type Waiters = Vec<oneshot::Sender<Result<StreamInfo, String>>>;

#[derive(Default)]
struct Inner {
    cache: HashMap<String, (StreamInfo, Instant)>,
    inflight: HashMap<String, Waiters>,
    /// Prefetches waiting for their turn.
    queued: HashSet<String>,
}

/// yt-dlp runs at once for background work (prefetch, durations). Several
/// parallel runs slow down the one a song is waiting for.
const BACKGROUND_RESOLVES: usize = 1;

#[derive(Clone)]
pub struct Resolver {
    yt_dlp: Option<PathBuf>,
    fake: bool,
    inner: Arc<Mutex<Inner>>,
    background: Arc<tokio::sync::Semaphore>,
}

impl Resolver {
    pub fn new(yt_dlp: Option<PathBuf>, fake: bool) -> Self {
        Self {
            yt_dlp,
            fake,
            inner: Arc::default(),
            background: Arc::new(tokio::sync::Semaphore::new(BACKGROUND_RESOLVES)),
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
    /// Cheap to call often: does nothing while the URL is cached or coming.
    pub fn prefetch(&self, youtube_id: &str) {
        {
            let mut inner = self.inner.lock().unwrap();
            let cached = inner
                .cache
                .get(youtube_id)
                .is_some_and(|(_, at)| at.elapsed() < CACHE_TTL);
            if cached
                || inner.inflight.contains_key(youtube_id)
                || !inner.queued.insert(youtube_id.to_string())
            {
                return;
            }
        }
        let me = self.clone();
        let id = youtube_id.to_string();
        tokio::spawn(async move {
            let result = me.stream_info_background(&id).await;
            me.inner.lock().unwrap().queued.remove(&id);
            if let Err(e) = result {
                tracing::debug!(id, "prefetch failed: {e}");
            }
        });
    }

    /// Like [`Self::stream_info`], but queued behind other background work.
    /// A song that has to play now joins (or overtakes) it instead of waiting.
    /// The URL is also checked, since YouTube sometimes hands out one that
    /// is refused: better to find out now than when the song should start.
    pub async fn stream_info_background(&self, youtube_id: &str) -> Result<StreamInfo, String> {
        let _permit = self.background.acquire().await;
        let info = self.stream_info(youtube_id).await?;
        if self.fake || !info.url.starts_with("https://") {
            return Ok(info);
        }
        let url = info.url.clone();
        let works = tokio::task::spawn_blocking(move || url_works(&url))
            .await
            .unwrap_or(true);
        if works {
            return Ok(info);
        }
        tracing::warn!(id = youtube_id, "stream URL is refused, resolving again");
        self.invalidate(youtube_id);
        self.stream_info(youtube_id).await
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
                video_url: None,
            });
        }
        // One `-J` run yields the stream URLs and the duration.
        let out = self
            .run(
                &[
                    "-J",
                    "-f",
                    FORMAT,
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
                    &["-J", "-f", FORMAT, "--no-playlist", "--no-warnings", url],
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

    /// Searches YouTube for videos or playlists (titles, channels and
    /// durations only).
    pub async fn search(&self, query: &str, playlists: bool) -> Result<Vec<SearchHit>, String> {
        if self.yt_dlp.is_none() && self.fake {
            return Ok((0..5)
                .map(|i| SearchHit {
                    youtube_id: if playlists {
                        format!("PLfakesearch{i}")
                    } else {
                        format!("fakesearch{i}")
                    },
                    title: format!("{query} {}", i + 1),
                    channel: Some("Test".into()),
                    duration_ms: (!playlists).then_some(FAKE_DURATION_MS),
                    playlist: playlists,
                    thumb_id: None,
                })
                .collect());
        }
        let target = if playlists {
            // YouTube's own results page with the "playlists only" filter.
            format!(
                "https://www.youtube.com/results?search_query={}&sp=EgIQAw%253D%253D",
                url_encode(query)
            )
        } else {
            format!("ytsearch{SEARCH_RESULTS}:{query}")
        };
        let end = SEARCH_RESULTS.to_string();
        let json = self
            .run(
                &[
                    "--flat-playlist",
                    "-J",
                    "--no-warnings",
                    "--playlist-end",
                    &end,
                    &target,
                ],
                SEARCH_TIMEOUT,
            )
            .await?;
        let v: Value =
            serde_json::from_str(&json).map_err(|_| strings::SEARCH_FAILED.to_string())?;
        Ok(if playlists {
            parse_playlist_search(&v)
        } else {
            parse_search(&v)
        })
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

/// Asks for the first two bytes. Only a clear refusal (4xx) counts as
/// broken; a network hiccup does not.
fn url_works(url: &str) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(OEMBED_TIMEOUT))
        .build()
        .new_agent();
    let r = agent.get(url).header("Range", "bytes=0-1").call();
    !matches!(r, Err(ureq::Error::StatusCode(c)) if (400..500).contains(&c))
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

/// Parses `yt-dlp -J -f <FORMAT>` output: either one format (its URL at the
/// top level) or a video+audio pair in `requested_formats`.
pub fn parse_stream_info(v: &Value) -> Option<StreamInfo> {
    let http = |f: &Value| -> Option<String> {
        f.get("url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("http"))
            .map(String::from)
    };
    let codec = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let pair = v.get("requested_formats").and_then(Value::as_array);
    let audio = pair.and_then(|p| {
        p.iter()
            .find(|f| codec(f, "vcodec") == "none" && codec(f, "acodec") != "none")
            .and_then(http)
    });
    let video_url = pair.and_then(|p| {
        p.iter()
            .find(|f| {
                codec(f, "acodec") == "none" && !matches!(codec(f, "vcodec").as_str(), "" | "none")
            })
            .and_then(http)
    });
    let url = audio.or_else(|| http(v))?;
    Some(StreamInfo {
        url,
        duration_ms: duration_ms(v),
        video_url,
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

/// Parses `yt-dlp --flat-playlist -J ytsearchN:...` output.
pub fn parse_search(v: &Value) -> Vec<SearchHit> {
    let Some(entries) = v.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?;
            if !shared::youtube::is_valid_video_id(id) {
                return None;
            }
            // Live streams never end, so they cannot be queued.
            let live = e.get("live_status").and_then(Value::as_str);
            if matches!(live, Some("is_live" | "is_upcoming")) {
                return None;
            }
            let title = e.get("title")?.as_str()?.trim();
            if title.is_empty() {
                return None;
            }
            let channel = ["channel", "uploader"]
                .iter()
                .find_map(|k| e.get(*k).and_then(Value::as_str))
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(String::from);
            Some(SearchHit {
                youtube_id: id.to_string(),
                title: title.to_string(),
                channel,
                duration_ms: duration_ms(e),
                playlist: false,
                thumb_id: Some(id.to_string()),
            })
        })
        .collect()
}

/// Parses `yt-dlp --flat-playlist -J` output of a playlist-filtered
/// results page.
pub fn parse_playlist_search(v: &Value) -> Vec<SearchHit> {
    let Some(entries) = v.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| {
            let url = e.get("url").and_then(Value::as_str).unwrap_or("");
            let Some(YoutubeRef::Playlist(id)) = parse_youtube_url(url) else {
                return None;
            };
            let title = e.get("title")?.as_str()?.trim();
            if title.is_empty() {
                return None;
            }
            let channel = ["channel", "uploader"]
                .iter()
                .find_map(|k| e.get(*k).and_then(Value::as_str))
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(String::from);
            let thumb_id = e
                .get("thumbnails")
                .and_then(Value::as_array)
                .and_then(|t| t.iter().find_map(|t| t.get("url")?.as_str()))
                .and_then(thumb_video_id);
            Some(SearchHit {
                youtube_id: id,
                title: title.to_string(),
                channel,
                duration_ms: None,
                playlist: true,
                thumb_id,
            })
        })
        .collect()
}

/// `https://i.ytimg.com/vi/<id>/hq720.jpg?...` → `<id>`.
fn thumb_video_id(url: &str) -> Option<String> {
    let rest = url.split("/vi/").nth(1)?;
    let id = rest.split('/').next()?;
    shared::youtube::is_valid_video_id(id).then(|| id.to_string())
}

/// Percent-encodes a search query for a URL.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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
    fn parses_search_json() {
        let v: Value = serde_json::from_str(
            r#"{"_type":"playlist","entries":[
                {"id":"1kHwMPKjDe0","title":"RASTA - MATEMATIKA","duration":201.0,"channel":"Balkaton"},
                {"id":"liveliveliv","title":"Radio","duration":null,"live_status":"is_live"},
                {"id":"bad","title":"x"},
                {"id":"KOGOPKRQ_qU","title":"Tekst","uploader":"Dj"}]}"#,
        )
        .unwrap();
        let hits = parse_search(&v);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].channel.as_deref(), Some("Balkaton"));
        assert_eq!(hits[0].duration_ms, Some(201_000));
        assert_eq!(hits[1].channel.as_deref(), Some("Dj"));
        assert_eq!(hits[1].duration_ms, None);
        assert!(parse_search(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parses_playlist_search_json() {
        let v: Value = serde_json::from_str(
            r#"{"_type":"playlist","entries":[
                {"title":"BALKAN HITS","channel":"Balkan Music","ie_key":"YoutubeTab",
                 "id":"PLToeUUa9hjL2FrcLwjcqOM5g69G3gqnpQ",
                 "url":"https://www.youtube.com/playlist?list=PLToeUUa9hjL2FrcLwjcqOM5g69G3gqnpQ",
                 "thumbnails":[{"url":"https://i.ytimg.com/vi/doedUH-xATc/hq720.jpg?sqp=x"}]},
                {"title":"A video","id":"dQw4w9WgXcQ","url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ"},
                {"title":"","url":"https://www.youtube.com/playlist?list=PLxNwmlAj-awdk7n0Bd28CSY4_O1yqsLY2"}]}"#,
        )
        .unwrap();
        let hits = parse_playlist_search(&v);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].playlist);
        assert_eq!(hits[0].youtube_id, "PLToeUUa9hjL2FrcLwjcqOM5g69G3gqnpQ");
        assert_eq!(hits[0].thumb_id.as_deref(), Some("doedUH-xATc"));
        assert_eq!(hits[0].channel.as_deref(), Some("Balkan Music"));
        assert!(hits[0]
            .url()
            .ends_with("list=PLToeUUa9hjL2FrcLwjcqOM5g69G3gqnpQ"));
        assert_eq!(url_encode("ceca & 2026"), "ceca+%26+2026");
        assert_eq!(url_encode("đ"), "%C4%91");
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
        assert_eq!(s.video_url, None);
        let v: Value = serde_json::from_str(r#"{"id":"dQw4w9WgXcQ"}"#).unwrap();
        assert!(parse_stream_info(&v).is_none());
        let v: Value = serde_json::from_str(
            r#"{"id":"dQw4w9WgXcQ","duration":213,"format_id":"137+251","requested_formats":[
                {"format_id":"137","vcodec":"avc1.640028","acodec":"none","url":"https://v.googlevideo.com/video"},
                {"format_id":"251","vcodec":"none","acodec":"opus","url":"https://v.googlevideo.com/audio"}]}"#,
        )
        .unwrap();
        let s = parse_stream_info(&v).unwrap();
        assert_eq!(s.url, "https://v.googlevideo.com/audio");
        assert_eq!(
            s.video_url.as_deref(),
            Some("https://v.googlevideo.com/video")
        );
    }
}
