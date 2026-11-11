//! File locations for config, logs and downloaded tools:
//! `%APPDATA%\SyncTube\` on Windows, `~/.config/synctube/` on Linux.

use std::path::PathBuf;

/// `%APPDATA%\SyncTube` (falls back to the current directory).
#[cfg(windows)]
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("SyncTube")
}

/// `$XDG_CONFIG_HOME/synctube`, else `~/.config/synctube`.
#[cfg(not(windows))]
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("synctube")
}

/// `<app dir>/bin` – where yt-dlp (and mpv on Windows) are downloaded.
pub fn bin_dir() -> PathBuf {
    app_dir().join("bin")
}

pub fn config_file() -> PathBuf {
    app_dir().join("config.json")
}

pub fn log_file() -> PathBuf {
    app_dir().join("synctube.log")
}

/// Directory containing the running executable.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

/// Finds a tool next to the exe, in the bin dir, or on `PATH`.
pub fn find_tool(file_name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(d) = exe_dir() {
        candidates.push(d.join(file_name));
    }
    candidates.push(bin_dir().join(file_name));
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(file_name)));
    }
    candidates.into_iter().find(|p| {
        p.metadata()
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    })
}

#[cfg(windows)]
pub const YTDLP_EXE: &str = "yt-dlp.exe";
#[cfg(windows)]
pub const MPV_EXE: &str = "mpv.exe";
#[cfg(not(windows))]
pub const YTDLP_EXE: &str = "yt-dlp";
#[cfg(not(windows))]
pub const MPV_EXE: &str = "mpv";

/// Paths to the external tools.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub yt_dlp: Option<PathBuf>,
    pub mpv: Option<PathBuf>,
}

impl Tools {
    pub fn discover() -> Self {
        Self {
            yt_dlp: find_tool(YTDLP_EXE),
            mpv: find_tool(MPV_EXE),
        }
    }
}
