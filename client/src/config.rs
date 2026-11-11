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
    /// Loudness normalization, so every song plays at about the same volume.
    pub normalize: bool,
    /// Rooms joined before, newest first (at most [`MAX_RECENT`]).
    pub recent: Vec<RecentRoom>,
}

/// How many recent rooms are remembered.
pub const MAX_RECENT: usize = 8;

/// A room joined before, for one-click rejoin. Stored only in the local config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RecentRoom {
    pub server_url: String,
    pub room: String,
    pub name: String,
    pub password: String,
}

impl RecentRoom {
    /// Same server, room and nickname (the password may have changed).
    fn same_place(&self, other: &RecentRoom) -> bool {
        self.server_url == other.server_url && self.room == other.room && self.name == other.name
    }

    /// The server's host, for display (`wss://host/ws` → `host`).
    pub fn server_host(&self) -> &str {
        let s = self.server_url.as_str();
        let s = s
            .strip_prefix("wss://")
            .or_else(|| s.strip_prefix("ws://"))
            .unwrap_or(s);
        s.split('/').next().unwrap_or(s)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            room: String::new(),
            password: String::new(),
            name: String::new(),
            volume: 70.0,
            normalize: true,
            recent: Vec::new(),
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

    /// Moves the current server/room/nickname to the top of the recent list.
    pub fn remember_current(&mut self) {
        let entry = RecentRoom {
            server_url: self.server_url.clone(),
            room: self.room.clone(),
            name: self.name.clone(),
            password: self.password.clone(),
        };
        self.recent.retain(|r| !r.same_place(&entry));
        self.recent.insert(0, entry);
        self.recent.truncate(MAX_RECENT);
    }

    /// Fills the connect fields from a recent entry.
    pub fn use_recent(&mut self, r: &RecentRoom) {
        self.server_url = r.server_url.clone();
        self.room = r.room.clone();
        self.name = r.name.clone();
        self.password = r.password.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(room: &str, name: &str) -> Config {
        Config {
            server_url: "wss://example.com/ws".into(),
            room: room.into(),
            name: name.into(),
            ..Config::default()
        }
    }

    #[test]
    fn recent_newest_first_without_duplicates() {
        let mut c = cfg("games", "jare");
        c.remember_current();
        c.room = "muzika".into();
        c.remember_current();
        c.room = "games".into();
        c.password = "nova".into();
        c.remember_current();
        let rooms: Vec<_> = c.recent.iter().map(|r| r.room.as_str()).collect();
        assert_eq!(rooms, ["games", "muzika"]);
        assert_eq!(c.recent[0].password, "nova");
        // A different nickname in the same room is its own entry.
        c.name = "drugi".into();
        c.remember_current();
        assert_eq!(c.recent.len(), 3);
    }

    #[test]
    fn recent_is_capped() {
        let mut c = cfg("x", "jare");
        for i in 0..MAX_RECENT + 3 {
            c.room = format!("soba{i}");
            c.remember_current();
        }
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent[0].room, format!("soba{}", MAX_RECENT + 2));
    }

    #[test]
    fn use_recent_fills_fields() {
        let mut c = cfg("games", "jare");
        c.password = "tajna".into();
        c.remember_current();
        let mut other = Config::default();
        other.use_recent(&c.recent[0]);
        assert_eq!(
            (
                other.server_url.as_str(),
                other.room.as_str(),
                other.name.as_str(),
                other.password.as_str()
            ),
            ("wss://example.com/ws", "games", "jare", "tajna")
        );
    }

    #[test]
    fn old_config_without_recent_still_loads() {
        let c: Config = serde_json::from_str(r#"{"room":"games","name":"jare"}"#).unwrap();
        assert!(c.recent.is_empty());
        assert_eq!(c.room, "games");
        assert!(c.normalize, "normalization is on by default");
    }

    #[test]
    fn server_host_strips_scheme_and_path() {
        let r = RecentRoom {
            server_url: "wss://example.com/ws".into(),
            ..RecentRoom::default()
        };
        assert_eq!(r.server_host(), "example.com");
    }
}
