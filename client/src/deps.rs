//! First-run download of yt-dlp and mpv into `<app dir>/bin/`.
//! On Linux only yt-dlp is downloaded; mpv comes from the system package.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::paths::{self, Tools, MPV_EXE, YTDLP_EXE};

#[cfg(windows)]
pub const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(all(not(windows), target_arch = "aarch64"))]
pub const YTDLP_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64";
#[cfg(all(not(windows), not(target_arch = "aarch64")))]
pub const YTDLP_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";
/// mpv.io links to these builds as the official Windows builds.
pub const MPV_RELEASES_API: &str =
    "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest";
const USER_AGENT: &str = "SyncTube";

#[derive(Debug, Clone, PartialEq)]
pub enum DepPhase {
    Waiting,
    Downloading { done: u64, total: Option<u64> },
    Extracting,
    Verifying,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct DepStatus {
    pub name: &'static str,
    pub phase: DepPhase,
}

#[derive(Debug, Clone, Default)]
pub struct DepsProgress {
    pub items: Vec<DepStatus>,
    /// Set when everything finished (successfully or not).
    pub finished: Option<Result<Tools, String>>,
}

pub type Shared = Arc<Mutex<DepsProgress>>;
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// True if a tool is missing and must be downloaded.
pub fn needed(tools: &Tools) -> bool {
    tools.yt_dlp.is_none() || tools.mpv.is_none()
}

/// Starts downloading the missing tools on a background thread.
pub fn start(tools: Tools, notify: Notify) -> Shared {
    let mut items = Vec::new();
    if tools.yt_dlp.is_none() {
        items.push(DepStatus {
            name: "yt-dlp",
            phase: DepPhase::Waiting,
        });
    }
    if tools.mpv.is_none() {
        items.push(DepStatus {
            name: "mpv",
            phase: DepPhase::Waiting,
        });
    }
    let shared: Shared = Arc::new(Mutex::new(DepsProgress {
        items,
        finished: None,
    }));
    let s = shared.clone();
    std::thread::spawn(move || {
        let result = run(tools, &s, &notify);
        if let Err(e) = &result {
            tracing::error!("dependency download failed: {e}");
        }
        s.lock().unwrap().finished = Some(result);
        notify();
    });
    shared
}

fn set_phase(shared: &Shared, notify: &Notify, name: &str, phase: DepPhase) {
    if let Some(item) = shared
        .lock()
        .unwrap()
        .items
        .iter_mut()
        .find(|i| i.name == name)
    {
        item.phase = phase;
    }
    notify();
}

fn run(mut tools: Tools, shared: &Shared, notify: &Notify) -> Result<Tools, String> {
    let bin = paths::bin_dir();
    fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let agent = agent();
    if tools.yt_dlp.is_none() {
        let r = install_ytdlp(&agent, &bin, shared, notify);
        set_phase(shared, notify, "yt-dlp", phase_of(&r));
        tools.yt_dlp = Some(r?);
    }
    if tools.mpv.is_none() {
        let r = install_mpv(&agent, &bin, shared, notify);
        set_phase(shared, notify, "mpv", phase_of(&r));
        tools.mpv = Some(r?);
    }
    Ok(tools)
}

fn phase_of(r: &Result<PathBuf, String>) -> DepPhase {
    match r {
        Ok(_) => DepPhase::Done,
        Err(e) => DepPhase::Failed(e.clone()),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .user_agent(USER_AGENT)
        .build()
        .new_agent()
}

fn install_ytdlp(
    agent: &ureq::Agent,
    bin: &Path,
    shared: &Shared,
    notify: &Notify,
) -> Result<PathBuf, String> {
    let dest = bin.join(YTDLP_EXE);
    download(agent, YTDLP_URL, &dest, |done, total| {
        set_phase(
            shared,
            notify,
            "yt-dlp",
            DepPhase::Downloading { done, total },
        )
    })?;
    make_executable(&dest)?;
    set_phase(shared, notify, "yt-dlp", DepPhase::Verifying);
    verify(&dest)?;
    Ok(dest)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Linux: mpv must come from the distribution's package manager.
#[cfg(not(windows))]
fn install_mpv(
    _agent: &ureq::Agent,
    _bin: &Path,
    _shared: &Shared,
    _notify: &Notify,
) -> Result<PathBuf, String> {
    Err(crate::strings::MPV_INSTALL_LINUX.into())
}

#[cfg(windows)]
fn install_mpv(
    agent: &ureq::Agent,
    bin: &Path,
    shared: &Shared,
    notify: &Notify,
) -> Result<PathBuf, String> {
    let url = latest_mpv_asset(agent)?;
    tracing::info!("mpv archive: {url}");
    let archive = bin.join("mpv-download.7z");
    download(agent, &url, &archive, |done, total| {
        set_phase(shared, notify, "mpv", DepPhase::Downloading { done, total })
    })?;
    set_phase(shared, notify, "mpv", DepPhase::Extracting);
    let dest = bin.join(MPV_EXE);
    let extracted = extract_mpv(&archive, &dest);
    let _ = fs::remove_file(&archive);
    extracted?;
    set_phase(shared, notify, "mpv", DepPhase::Verifying);
    verify(&dest)?;
    Ok(dest)
}

/// Finds the newest plain x86-64 build (not the v3/dev/ffmpeg variants).
fn latest_mpv_asset(agent: &ureq::Agent) -> Result<String, String> {
    let body = agent
        .get(MPV_RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("GitHub: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("GitHub: {e}"))?;
    let assets = v["assets"].as_array().ok_or("GitHub: no assets")?;
    assets
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            is_plain_mpv_asset(name).then(|| url.to_string())
        })
        .next()
        .ok_or_else(|| "GitHub: no mpv x86_64 build in latest release".into())
}

/// `mpv-x86_64-20261109-git-abc1234.7z`
pub fn is_plain_mpv_asset(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mpv-x86_64-") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".7z") else {
        return false;
    };
    let mut parts = rest.splitn(3, '-');
    let date = parts.next().unwrap_or("");
    let git = parts.next().unwrap_or("");
    let hash = parts.next().unwrap_or("");
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && git == "git"
        && !hash.is_empty()
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Downloads to `dest` via a temporary file, reporting progress.
fn download(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let mut resp = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let part = dest.with_extension("part");
    let mut file = fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_report = 0u64;
    progress(0, total);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("{}: {e}", part.display()))?;
        done += n as u64;
        if done - last_report >= 256 * 1024 {
            last_report = done;
            progress(done, total);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    progress(done, total);
    if done == 0 || total.is_some_and(|t| t != done) {
        let _ = fs::remove_file(&part);
        return Err(format!("{url}: incomplete download"));
    }
    let _ = fs::remove_file(dest);
    fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Extracts only `mpv.exe` from the archive.
pub fn extract_mpv(archive: &Path, dest: &Path) -> Result<(), String> {
    let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty())
        .map_err(|e| format!("7z: {e}"))?;
    let name = reader
        .archive()
        .files
        .iter()
        .filter(|f| !f.is_directory())
        .map(|f| f.name().to_string())
        .find(|n| {
            let base = n.rsplit(['/', '\\']).next().unwrap_or(n);
            base.eq_ignore_ascii_case(MPV_EXE)
        })
        .ok_or("7z: mpv.exe not in archive")?;
    let data = reader.read_file(&name).map_err(|e| format!("7z: {e}"))?;
    if data.is_empty() {
        return Err("7z: mpv.exe is empty".into());
    }
    let part = dest.with_extension("part");
    fs::write(&part, &data).map_err(|e| format!("{}: {e}", part.display()))?;
    let _ = fs::remove_file(dest);
    fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Checks the file is non-empty and runs `--version`.
pub fn verify(exe: &Path) -> Result<(), String> {
    let len = fs::metadata(exe).map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Err(format!("{} is empty", exe.display()));
    }
    let out = crate::proc::std_command(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!("{} --version failed", exe.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_plain_build() {
        assert!(is_plain_mpv_asset("mpv-x86_64-20261109-git-1a2b3c4.7z"));
        assert!(!is_plain_mpv_asset("mpv-x86_64-v3-20261109-git-1a2b3c4.7z"));
        assert!(!is_plain_mpv_asset(
            "mpv-dev-x86_64-20261109-git-1a2b3c4.7z"
        ));
        assert!(!is_plain_mpv_asset("ffmpeg-x86_64-git-1a2b3c4.7z"));
        assert!(!is_plain_mpv_asset("mpv-i686-20261109-git-1a2b3c4.7z"));
        assert!(!is_plain_mpv_asset("mpv-x86_64-20261109-git-1a2b3c4.zip"));
    }
}
