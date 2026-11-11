//! Per-song loudness normalization: each stream is measured once (EBU R128,
//! in a separate silent mpv that decodes as fast as it can) and gets one
//! fixed gain for the whole song. Unlike a dynamic normalizer this never
//! reshapes the sound (bass, drops, punch); it only moves the level.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Level every song is brought to. Louder than YouTube's -14, so loud
/// songs are turned down less and keep their punch (bass fades first when
/// music gets quieter).
pub const TARGET_LUFS: f64 = -10.0;
/// A boost never pushes the true peak above this, so nothing clips.
const PEAK_CEILING_DB: f64 = -1.0;
const MAX_BOOST_DB: f64 = 12.0;
const MAX_CUT_DB: f64 = -24.0;
/// Only the start is scanned, so hour-long mixes stay quick.
const SCAN_SECONDS: u32 = 300;
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: f64,
    pub true_peak_db: Option<f64>,
}

impl Loudness {
    /// The fixed gain (dB) that brings this song to the target level.
    /// Loud songs are turned down fully; quiet ones are turned up only as
    /// far as their peaks allow.
    pub fn gain_db(&self) -> f64 {
        let mut g = TARGET_LUFS - self.integrated_lufs;
        if g > 0.0 {
            if let Some(peak) = self.true_peak_db {
                g = g.min((PEAK_CEILING_DB - peak).max(0.0));
            }
        }
        g.clamp(MAX_CUT_DB, MAX_BOOST_DB)
    }
}

/// Most songs remembered; the oldest half is dropped beyond this.
const STORE_MAX: usize = 20_000;

/// Measured songs, kept in `loudness.json` so each song is scanned only once
/// (not once per app start). Stores the measurement, not the gain, so a
/// changed target level still applies to old entries.
#[derive(Default)]
pub struct Store {
    path: Option<PathBuf>,
    songs: HashMap<String, (Loudness, u64)>,
    seq: u64,
}

impl Store {
    pub fn load(path: PathBuf) -> Self {
        let songs: HashMap<String, (Loudness, u64)> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let seq = songs.values().map(|(_, n)| *n).max().unwrap_or(0);
        Self {
            path: Some(path),
            songs,
            seq,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Loudness)> {
        self.songs.iter().map(|(id, (l, _))| (id, l))
    }

    pub fn insert(&mut self, youtube_id: &str, l: Loudness) {
        self.seq += 1;
        self.songs.insert(youtube_id.to_string(), (l, self.seq));
        if self.songs.len() > STORE_MAX {
            let cut = self.seq - (STORE_MAX / 2) as u64;
            self.songs.retain(|_, (_, n)| *n > cut);
        }
        let Some(path) = &self.path else { return };
        let Ok(json) = serde_json::to_vec(&self.songs) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// Reads the summary that ffmpeg's ebur128 filter logs at the end.
pub fn parse_summary(log: &str) -> Option<Loudness> {
    fn value(line: &str, key: &str, unit: &str) -> Option<f64> {
        line.strip_prefix(key)?
            .trim()
            .strip_suffix(unit)?
            .trim()
            .parse()
            .ok()
    }
    let mut integrated = None;
    let mut peak = None;
    for line in log.lines() {
        let line = line.trim_start_matches("[ffmpeg]").trim();
        if let Some(v) = value(line, "I:", "LUFS") {
            integrated = Some(v);
        } else if let Some(v) = value(line, "Peak:", "dBFS") {
            peak = Some(v);
        }
    }
    let integrated_lufs = integrated.filter(|v: &f64| v.is_finite())?;
    Some(Loudness {
        integrated_lufs,
        true_peak_db: peak.filter(|v: &f64| v.is_finite()),
    })
}

/// Measures a stream with mpv (no sound output). Takes a few seconds.
pub async fn measure(mpv: &Path, url: &str) -> Result<Loudness, String> {
    let mut cmd = crate::proc::command(mpv);
    cmd.args([
        "--no-config",
        "--no-video",
        "--ytdl=no",
        "--ao=null",
        "--ao-null-untimed",
        &format!("--length={SCAN_SECONDS}"),
        "--af=lavfi=[ebur128=peak=true:framelog=quiet]",
        "--msg-level=all=no,ffmpeg=v",
        url,
    ]);
    let child = cmd.spawn().map_err(|e| format!("mpv: {e}"))?;
    crate::proc::bind_to_job(&child);
    let out = tokio::time::timeout(SCAN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "loudness scan timed out".to_string())?
        .map_err(|e| format!("mpv: {e}"))?;
    let log = format!(
        "{}
{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_summary(&log).ok_or_else(|| "no loudness summary from mpv".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_survives_restart() {
        let dir = std::env::temp_dir().join(format!("synctube-loud-{}", std::process::id()));
        let path = dir.join("sub").join("loudness.json");
        let mut s = Store::load(path.clone());
        assert_eq!(s.iter().count(), 0);
        s.insert("1kHwMPKjDe0", l(-8.1, Some(-0.5)));
        s.insert("KOGOPKRQ_qU", l(-9.0, None));
        let s = Store::load(path);
        let got: HashMap<_, _> = s.iter().map(|(k, v)| (k.clone(), *v)).collect();
        assert_eq!(got.len(), 2);
        assert_eq!(got["1kHwMPKjDe0"], l(-8.1, Some(-0.5)));
        let _ = std::fs::remove_dir_all(dir);
    }

    const LOG: &str = "[ffmpeg] Parsed_ebur128_0: Summary:
[ffmpeg]
[ffmpeg]   Integrated loudness:
[ffmpeg]     I:          -5.2 LUFS
[ffmpeg]     Threshold: -15.6 LUFS
[ffmpeg]
[ffmpeg]   Loudness range:
[ffmpeg]     LRA:         8.6 LU
[ffmpeg]     Threshold: -25.7 LUFS
[ffmpeg]     LRA low:   -12.5 LUFS
[ffmpeg]     LRA high:   -3.9 LUFS
[ffmpeg]
[ffmpeg]   True peak:
[ffmpeg]     Peak:        4.5 dBFS
";

    fn l(i: f64, peak: Option<f64>) -> Loudness {
        Loudness {
            integrated_lufs: i,
            true_peak_db: peak,
        }
    }

    #[test]
    fn parses_ebur128_summary() {
        assert_eq!(parse_summary(LOG), Some(l(-5.2, Some(4.5))));
        assert_eq!(parse_summary("nothing here"), None);
        assert_eq!(
            parse_summary("[ffmpeg]     I:         -27.0 LUFS\n"),
            Some(l(-27.0, None))
        );
    }

    #[test]
    fn loud_song_is_turned_down_fully() {
        // Peaks above 0 dBFS do not matter when cutting.
        assert!((l(-5.2, Some(4.5)).gain_db() - -4.8).abs() < 1e-9);
        assert!((l(-10.0, Some(2.0)).gain_db()).abs() < 1e-9);
    }

    #[test]
    fn quiet_song_boost_is_limited_by_peaks() {
        // Wants +16.2, but peaks at -7.2 dBTP leave room for +6.2 only.
        assert!((l(-26.2, Some(-7.2)).gain_db() - 6.2).abs() < 1e-9);
        // Enough headroom: full boost.
        assert!((l(-16.0, Some(-12.0)).gain_db() - 6.0).abs() < 1e-9);
        // Already peaking at the ceiling: no boost, never a cut.
        assert_eq!(l(-20.0, Some(0.5)).gain_db(), 0.0);
    }

    #[test]
    fn gain_is_clamped() {
        assert_eq!(l(-70.0, None).gain_db(), MAX_BOOST_DB);
        assert_eq!(l(20.0, None).gain_db(), MAX_CUT_DB);
    }
}
