//! Logging to `%APPDATA%\SyncTube\synctube.log` (the GUI has no console).

use std::fs::OpenOptions;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Log files larger than this are truncated on start.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Logs to the log file; if `stderr` is set, to stderr instead.
pub fn init(stderr: bool) {
    let filter = EnvFilter::try_from_env("SYNCTUBE_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,client=debug"));
    if stderr {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
        return;
    }
    let path = crate::paths::log_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    let file = OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(&path);
    if let Ok(file) = file {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .try_init();
    }
}
