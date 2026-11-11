//! Persistent client settings (`%APPDATA%\SyncTube\config.json`).

use serde::{Deserialize, Serialize};

/// Server URL prefilled on first run. Empty by default, so public builds do not
/// carry anyone's server address; a private build can bake one in with
/// `SYNCTUBE_SERVER_URL=wss://<name>.onrender.com/ws` (see `build-release.ps1`).
/// The address the user enters is remembered in the config file.
pub const DEFAULT_SERVER_URL: &str = match option_env!("SYNCTUBE_SERVER_URL") {
    Some(url) => url,
    None => "",
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub server_url: String,
    pub room: String,
    pub password: String,
    pub name: String,
    /// 0–100.
    pub volume: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            room: String::new(),
            password: String::new(),
            name: String::new(),
            volume: 70.0,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(crate::paths::config_file())
            .ok()
            // Tolerate a BOM from hand-edited files (Notepad/PowerShell add one).
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = crate::paths::config_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, s).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
            Err(e) => tracing::warn!("cannot serialize config: {e}"),
        }
    }
}
