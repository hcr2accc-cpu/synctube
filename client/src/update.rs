//! In-app update: finds the newest `vX.Y` release on GitHub, downloads this
//! platform's build next to the running program, swaps the files and starts
//! the new version. The server is not involved.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::deps;

const RELEASES_API: &str =
    "https://api.github.com/repos/hcr2accc-cpu/synctube/releases?per_page=20";
/// Where people can download by hand when the update can't install itself.
pub const RELEASES_PAGE: &str = "https://github.com/hcr2accc-cpu/synctube/releases/latest";

#[cfg(windows)]
const ASSET: &str = "synctube.exe";
#[cfg(not(windows))]
const ASSET: &str = "synctube-linux-x86_64.tar.gz";

/// A newer release with a build for this platform.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub tag: String,
    pub url: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Phase {
    #[default]
    Checking,
    /// Nothing newer, or GitHub could not be asked (no message then).
    UpToDate,
    Available(Update),
    Downloading {
        tag: String,
        done: u64,
        total: Option<u64>,
    },
    Installing(String),
    /// The new version was started; this one should close.
    Restarting,
    Failed(String),
}

pub type Shared = Arc<Mutex<Phase>>;

/// Asks GitHub for a newer version on a background thread.
pub fn start_check(notify: deps::Notify) -> Shared {
    let shared: Shared = Arc::default();
    let s = shared.clone();
    std::thread::spawn(move || {
        let phase = match check() {
            Ok(Some(u)) => {
                tracing::info!(tag = %u.tag, "update available");
                Phase::Available(u)
            }
            Ok(None) => Phase::UpToDate,
            Err(e) => {
                tracing::info!("update check failed: {e}");
                Phase::UpToDate
            }
        };
        *s.lock().unwrap() = phase;
        notify();
    });
    shared
}

/// Downloads and installs `u` on a background thread.
pub fn start_install(shared: &Shared, u: Update, notify: deps::Notify) {
    *shared.lock().unwrap() = Phase::Downloading {
        tag: u.tag.clone(),
        done: 0,
        total: u.size,
    };
    notify();
    let s = shared.clone();
    std::thread::spawn(move || {
        let phase = match install(&u, &s, &notify) {
            Ok(()) => Phase::Restarting,
            Err(e) => {
                tracing::error!("update failed: {e}");
                Phase::Failed(e)
            }
        };
        *s.lock().unwrap() = phase;
        notify();
    });
}

/// Removes what an earlier update left behind (the old program).
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let old = with_suffix(&exe, ".old");
    if old.exists() {
        // The old program may still be closing.
        std::thread::spawn(move || {
            for _ in 0..20 {
                if std::fs::remove_file(&old).is_ok() || !old.exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    }
}

fn check() -> Result<Option<Update>, String> {
    let body = deps::agent()
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("GitHub: {e}"))?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("GitHub: {e}"))?;
    Ok(pick(&v, env!("CARGO_PKG_VERSION"), ASSET))
}

/// `v0.7`, `0.7.1` -> (0, 7, 1). Anything else (`windows`, `v1-beta`) is None.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut next = |required: bool| match parts.next() {
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
        None if !required => Some(0),
        _ => None,
    };
    let v = (next(true)?, next(true)?, next(false)?);
    parts.next().is_none().then_some(v)
}

/// The newest proper release above `current` that has `asset`.
pub fn pick(releases: &Value, current: &str, asset: &str) -> Option<Update> {
    let current = parse_version(current)?;
    releases
        .as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter(|r| !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = parse_version(tag)?;
            let a = r["assets"]
                .as_array()?
                .iter()
                .find(|a| a["name"].as_str() == Some(asset))?;
            let update = Update {
                tag: tag.to_string(),
                url: a["browser_download_url"].as_str()?.to_string(),
                size: a["size"].as_u64(),
            };
            Some((version, update))
        })
        .filter(|(v, _)| *v > current)
        .max_by_key(|(v, _)| *v)
        .map(|(_, u)| u)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn install(u: &Update, shared: &Shared, notify: &deps::Notify) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    replace(&exe, u, shared, notify)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(&exe)
        .args(args)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    tracing::info!(tag = %u.tag, "update installed, new version started");
    Ok(())
}

/// Downloads `u` and puts it in place of `exe`.
fn replace(exe: &Path, u: &Update, shared: &Shared, notify: &deps::Notify) -> Result<(), String> {
    let new = with_suffix(exe, ".new");
    let download = with_suffix(exe, ".download");
    let tag = u.tag.clone();
    deps::download(&deps::agent(), &u.url, &download, |done, total| {
        *shared.lock().unwrap() = Phase::Downloading {
            tag: tag.clone(),
            done,
            total,
        };
        notify();
    })?;
    *shared.lock().unwrap() = Phase::Installing(u.tag.clone());
    notify();
    let len = std::fs::metadata(&download).map(|m| m.len()).unwrap_or(0);
    if u.size.is_some_and(|s| s != len) {
        let _ = std::fs::remove_file(&download);
        return Err(format!("{}: incomplete download", u.url));
    }
    let prepared = unpack(&download, &new);
    let _ = std::fs::remove_file(&download);
    prepared?;
    swap(exe, &new)
}

/// Windows: the download is the program itself.
#[cfg(windows)]
fn unpack(download: &Path, dest: &Path) -> Result<(), String> {
    let mut head = [0u8; 2];
    use std::io::Read;
    std::fs::File::open(download)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| e.to_string())?;
    if &head != b"MZ" {
        return Err("download is not a Windows program".into());
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(download, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Linux: the program is `synctube/synctube` inside the archive.
#[cfg(not(windows))]
fn unpack(download: &Path, dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = with_suffix(dest, ".d");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(download)
        .arg("-C")
        .arg(&dir)
        .output()
        .map_err(|e| format!("tar: {e}"))?;
    let program = dir.join("synctube").join("synctube");
    let result = if !out.status.success() || !program.is_file() {
        Err("tar: synctube not in the archive".to_string())
    } else {
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&program, dest)
            .and_then(|_| std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755)))
            .map_err(|e| format!("{}: {e}", dest.display()))
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Puts `new` where `exe` is. A running program can be renamed (not
/// overwritten), so the old one moves aside to `.old` first.
fn swap(exe: &Path, new: &Path) -> Result<(), String> {
    let old = with_suffix(exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| {
        let _ = std::fs::remove_file(new);
        format!("{}: {e}", exe.display())
    })?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(new);
        return Err(format!("{}: {e}", exe.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.7"), Some((0, 7, 0)));
        assert_eq!(parse_version("0.6.0"), Some((0, 6, 0)));
        assert_eq!(parse_version("v1.10.2"), Some((1, 10, 2)));
        assert_eq!(parse_version("windows"), None);
        assert_eq!(parse_version("v1"), None);
        assert_eq!(parse_version("v1.2-beta"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert!(parse_version("v0.10") > parse_version("v0.9"));
        assert!(parse_version("v0.7") > parse_version("0.6.0"));
    }

    fn release(tag: &str, assets: &[&str]) -> Value {
        serde_json::json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "assets": assets.iter().map(|n| serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://x/{tag}/{n}"),
                "size": 10,
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn picks_newest_with_asset() {
        let mut beta = release("v0.9", &["synctube.exe"]);
        beta["prerelease"] = true.into();
        let v = Value::Array(vec![
            release("windows", &["synctube.exe"]),
            release("v0.6", &["synctube.exe"]),
            release("v0.8", &["synctube.apk"]),
            release("v0.7", &["synctube.exe", "synctube.apk"]),
            beta,
        ]);
        let u = pick(&v, "0.6.0", "synctube.exe").unwrap();
        assert_eq!(u.tag, "v0.7");
        assert_eq!(u.url, "https://x/v0.7/synctube.exe");
        assert_eq!(u.size, Some(10));
        assert_eq!(pick(&v, "0.6.0", "synctube.apk").unwrap().tag, "v0.8");
        assert_eq!(pick(&v, "0.7.0", "synctube.exe"), None);
        assert_eq!(pick(&Value::Null, "0.6.0", "synctube.exe"), None);
    }

    #[test]
    fn swap_moves_old_aside() {
        let dir = std::env::temp_dir().join(format!("synctube-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("synctube.exe");
        let new = with_suffix(&exe, ".new");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"old");
        assert!(!new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Downloads the real newest release over a dummy program.
    /// `cargo test -p client --lib live_update -- --ignored`
    #[test]
    #[ignore]
    fn live_update() {
        let v: Value = serde_json::from_str(
            &deps::agent()
                .get(RELEASES_API)
                .call()
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap(),
        )
        .unwrap();
        let u = pick(&v, "0.0.1", ASSET).expect("a release with this platform's build");
        let dir = std::env::temp_dir().join(format!("synctube-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("synctube.exe");
        std::fs::write(&exe, b"old").unwrap();
        let shared: Shared = Arc::default();
        replace(&exe, &u, &shared, &(Arc::new(|| {}) as deps::Notify)).unwrap();
        let len = std::fs::metadata(&exe).unwrap().len();
        assert!(len > 1_000_000, "{len}");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
