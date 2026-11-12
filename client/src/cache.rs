//! Small on-disk cache of upcoming songs' audio, so a skip to the next song
//! loads from disk (almost instant) instead of the network.
//!
//! Downloads run one at a time, in small paced chunks, so they never compete
//! with the song that is playing or with the sync messages (ping).

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bytes per range request.
const CHUNK: u64 = 1024 * 1024;
/// Average download speed cap (bytes/s): far below a normal connection, so
/// the line stays free for the playing song and for low latency.
const RATE: f64 = 2.5 * 1024.0 * 1024.0;
/// Songs bigger than this are not cached (hour-long mixes).
const MAX_FILE: u64 = 60 * 1024 * 1024;
/// How many songs are kept.
const KEEP: usize = 6;
const TIMEOUT: Duration = Duration::from_secs(30);
const EXT: &str = "audio";

#[derive(Clone)]
pub struct AudioCache {
    dir: PathBuf,
    busy: Arc<Mutex<HashSet<String>>>,
}

impl AudioCache {
    /// Opens the cache in `dir`, dropping unfinished downloads.
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "part") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Self {
            dir,
            busy: Arc::default(),
        }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.{EXT}"))
    }

    /// The cached file for a song, if it is complete.
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        if !valid_id(id) || self.busy(id) {
            return None;
        }
        let p = self.file(id);
        std::fs::metadata(&p)
            .ok()
            .filter(|m| m.len() > 0)
            .map(|_| p)
    }

    pub fn busy(&self, id: &str) -> bool {
        self.busy.lock().unwrap().contains(id)
    }

    /// Deletes a cached song (e.g. mpv could not play it).
    pub fn forget(&self, id: &str) {
        if valid_id(id) {
            let _ = std::fs::remove_file(self.file(id));
        }
    }

    /// Downloads `url` as song `id` (blocking; run it off the async thread).
    pub fn download(&self, id: &str, url: &str) -> Result<(), String> {
        if !valid_id(id) || !url.starts_with("https://") {
            return Err("bad id or url".into());
        }
        if self.path(id).is_some() {
            return Ok(());
        }
        if !self.busy.lock().unwrap().insert(id.to_string()) {
            return Ok(());
        }
        let part = self.dir.join(format!("{id}.part"));
        let result = fetch(url, &part)
            .and_then(|()| std::fs::rename(&part, self.file(id)).map_err(|e| e.to_string()));
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        self.busy.lock().unwrap().remove(id);
        self.prune();
        result
    }

    /// Keeps the newest [`KEEP`] songs.
    fn prune(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == EXT))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        for (_, p) in files.into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn valid_id(id: &str) -> bool {
    shared::youtube::is_valid_video_id(id)
}

/// `bytes 0-1048575/3456789` → total size.
fn total_from_content_range(v: &str) -> Option<u64> {
    v.rsplit('/').next()?.trim().parse().ok()
}

fn fetch(url: &str, part: &Path) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(true)
        .build()
        .new_agent();
    let mut out = std::fs::File::create(part).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut done: u64 = 0;
    let mut total: Option<u64>;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let end = done + CHUNK - 1;
        let mut resp = agent
            .get(url)
            .header("Range", &format!("bytes={done}-{end}"))
            .call()
            .map_err(|e| e.to_string())?;
        let partial = resp.status().as_u16() == 206;
        if partial {
            total = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(total_from_content_range);
        } else {
            total = resp.body().content_length();
        }
        if total.is_some_and(|t| t > MAX_FILE) {
            return Err("too big".into());
        }
        let mut reader = resp.body_mut().with_config().limit(MAX_FILE).reader();
        let mut got: u64 = 0;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            done += n as u64;
            if done > MAX_FILE {
                return Err("too big".into());
            }
            // Pace: never faster than RATE on average.
            let ahead = done as f64 / RATE - started.elapsed().as_secs_f64();
            if ahead > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(ahead));
            }
        }
        let finished = match total {
            Some(t) => done >= t,
            None => !partial || got < CHUNK,
        };
        if !partial || finished {
            break;
        }
        if got == 0 {
            return Err("empty response".into());
        }
    }
    if total.is_some_and(|t| done < t) || done == 0 {
        return Err("incomplete".into());
    }
    out.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_total() {
        assert_eq!(
            total_from_content_range("bytes 0-1048575/3456789"),
            Some(3456789)
        );
        assert_eq!(total_from_content_range("bytes */*"), None);
    }

    #[test]
    fn path_only_for_finished_files() {
        let dir = std::env::temp_dir().join(format!("synctube-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = AudioCache::new(dir.clone());
        assert!(c.path("dQw4w9WgXcQ").is_none());
        std::fs::write(dir.join("dQw4w9WgXcQ.audio"), b"x").unwrap();
        std::fs::write(dir.join("aaaaaaaaaaa.part"), b"x").unwrap();
        assert!(c.path("dQw4w9WgXcQ").is_some());
        assert!(c.path("../evil").is_none());
        let c = AudioCache::new(dir.clone());
        assert!(
            !dir.join("aaaaaaaaaaa.part").exists(),
            "stale parts removed"
        );
        c.forget("dQw4w9WgXcQ");
        assert!(c.path("dQw4w9WgXcQ").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live {
    /// `SYNCTUBE_TEST_URL=<googlevideo url> cargo test -p client --lib cache::live -- --ignored`
    #[test]
    #[ignore]
    fn downloads_real_stream() {
        let Ok(url) = std::env::var("SYNCTUBE_TEST_URL") else {
            return;
        };
        let dir = std::env::temp_dir().join("synctube-cache-live");
        let _ = std::fs::remove_dir_all(&dir);
        let c = super::AudioCache::new(dir.clone());
        let t = std::time::Instant::now();
        c.download("dQw4w9WgXcQ", &url).unwrap();
        let p = c.path("dQw4w9WgXcQ").unwrap();
        eprintln!(
            "{} bytes in {:?}",
            std::fs::metadata(&p).unwrap().len(),
            t.elapsed()
        );
    }
}
