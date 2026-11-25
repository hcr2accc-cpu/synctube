//! The user's own playlists, what they listened to, and the picking and
//! ranking of recommendations. Everything stays on this computer
//! (`playlists.json` and `history.json`); no network here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shared::limits::{clamp_chars, MAX_QUEUE, MAX_TITLE_CHARS};
use shared::TrackInfo;
use uuid::Uuid;

/// Longest playlist name, in characters.
pub const MAX_NAME_CHARS: usize = 60;
/// Most songs in one playlist (a room's queue holds no more).
pub const MAX_PLAYLIST: usize = MAX_QUEUE;
/// How many recommendations are shown.
pub const MAX_RECS: usize = 30;
/// Recommendations are fetched again after this long.
const RECS_TTL_SECS: u64 = 12 * 60 * 60;
/// Most songs remembered; the weakest ones are forgotten beyond this.
const MAX_HISTORY: usize = 3000;

const HEARD: f64 = 1.0;
const HEARD_OWN: f64 = 2.5;
const IN_PLAYLIST: f64 = 2.0;
const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;
/// A song heard this recently is not recommended.
const RECENT_SECS: u64 = 7 * 24 * 3600;
const TOP_SEEDS: usize = 3;
const RANDOM_SEEDS: usize = 2;
const MIN_REC_MS: u64 = 60_000;
const MAX_REC_MS: u64 = 12 * 60_000;

/// A song counts as listened to after this long.
const LISTEN_MS: u64 = 30_000;
/// Longest step between two ticks that counts as listening.
const LISTEN_STEP: Duration = Duration::from_secs(2);

/// JSON size of one `AddResolved` batch (the server takes 64 KB messages).
const BATCH_BYTES: usize = 40 * 1024;

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    pub youtube_id: String,
    pub title: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub channel: Option<String>,
}

impl From<TrackInfo> for Song {
    fn from(t: TrackInfo) -> Self {
        Self {
            youtube_id: t.youtube_id,
            title: t.title,
            duration_ms: t.duration_ms,
            channel: None,
        }
    }
}

impl From<&shared::Track> for Song {
    fn from(t: &shared::Track) -> Self {
        Self {
            youtube_id: t.youtube_id.clone(),
            title: t.title.clone(),
            duration_ms: t.duration_ms,
            channel: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<Song>,
}

impl Playlist {
    pub fn contains(&self, youtube_id: &str) -> bool {
        self.tracks.iter().any(|t| t.youtube_id == youtube_id)
    }
}

/// What is known about a song the user listened to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Heard {
    title: String,
    duration_ms: Option<u64>,
    plays: u32,
    own_plays: u32,
    /// Taste score as of `last_unix`; halves every 14 days.
    score: f64,
    last_unix: u64,
}

impl Heard {
    fn score_at(&self, now: u64) -> f64 {
        let age = now.saturating_sub(self.last_unix) as f64;
        self.score * 0.5f64.powf(age / HALF_LIFE_SECS)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Recs {
    at_unix: u64,
    items: Vec<Song>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct History {
    songs: HashMap<String, Heard>,
    recs: Recs,
}

/// Outcome of adding songs to a playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Added {
    pub added: usize,
    /// Left out because the playlist is full.
    pub no_room: usize,
}

/// Playlists and history, saved on every change.
#[derive(Default)]
pub struct Library {
    playlists: Vec<Playlist>,
    history: History,
    playlists_file: Option<PathBuf>,
    history_file: Option<PathBuf>,
}

impl Library {
    /// Loads both files. Without a directory nothing is ever saved.
    pub fn load(dir: Option<&Path>) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        let (playlists, playlists_file) = read(dir.join("playlists.json"));
        let (history, history_file) = read(dir.join("history.json"));
        Self {
            playlists,
            history,
            playlists_file,
            history_file,
        }
    }

    pub fn playlists(&self) -> &[Playlist] {
        &self.playlists
    }

    pub fn get(&self, id: Uuid) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    /// Creates a playlist, or finds the one with the same name. `None` when
    /// the name is empty.
    pub fn create(&mut self, name: &str) -> Option<Uuid> {
        let name = clamp_chars(name, MAX_NAME_CHARS);
        if name.is_empty() {
            return None;
        }
        let key = name.to_lowercase();
        if let Some(p) = self.playlists.iter().find(|p| p.name.to_lowercase() == key) {
            return Some(p.id);
        }
        let id = Uuid::new_v4();
        self.playlists.push(Playlist {
            id,
            name,
            tracks: Vec::new(),
        });
        Some(id)
    }

    pub fn rename(&mut self, id: Uuid, name: &str) {
        let name = clamp_chars(name, MAX_NAME_CHARS);
        if name.is_empty() {
            return;
        }
        if let Some(p) = self.get_mut(id) {
            p.name = name;
        }
    }

    pub fn delete(&mut self, id: Uuid) {
        self.playlists.retain(|p| p.id != id);
    }

    /// Adds songs, skipping the ones already there. `None` when the playlist
    /// no longer exists.
    pub fn add(&mut self, id: Uuid, songs: impl IntoIterator<Item = Song>) -> Option<Added> {
        let p = self.get_mut(id)?;
        let mut have: HashSet<String> = p.tracks.iter().map(|t| t.youtube_id.clone()).collect();
        let mut out = Added::default();
        for mut s in songs {
            if !shared::youtube::is_valid_video_id(&s.youtube_id) || have.contains(&s.youtube_id) {
                continue;
            }
            if p.tracks.len() >= MAX_PLAYLIST {
                out.no_room += 1;
                continue;
            }
            s.title = clamp_chars(&s.title, MAX_TITLE_CHARS);
            have.insert(s.youtube_id.clone());
            p.tracks.push(s);
            out.added += 1;
        }
        Some(out)
    }

    pub fn remove(&mut self, id: Uuid, youtube_id: &str) {
        if let Some(p) = self.get_mut(id) {
            p.tracks.retain(|t| t.youtube_id != youtube_id);
        }
    }

    /// Moves a song to `to` (clamped to the list).
    pub fn move_song(&mut self, id: Uuid, youtube_id: &str, to: usize) {
        if let Some(p) = self.get_mut(id) {
            if let Some(from) = p.tracks.iter().position(|t| t.youtube_id == youtube_id) {
                let s = p.tracks.remove(from);
                p.tracks.insert(to.min(p.tracks.len()), s);
            }
        }
    }

    /// Fills in durations that were unknown when songs were saved. True when
    /// something changed.
    pub fn learn_durations(&mut self, queue: &[shared::Track]) -> bool {
        let missing = |t: &Song| t.duration_ms.is_none();
        if !self.playlists.iter().any(|p| p.tracks.iter().any(missing)) {
            return false;
        }
        let known: HashMap<&str, u64> = queue
            .iter()
            .filter_map(|t| Some((t.youtube_id.as_str(), t.duration_ms?)))
            .collect();
        let mut changed = false;
        for t in self.playlists.iter_mut().flat_map(|p| &mut p.tracks) {
            if missing(t) {
                if let Some(d) = known.get(t.youtube_id.as_str()) {
                    t.duration_ms = Some(*d);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn has_history(&self) -> bool {
        !self.history.songs.is_empty()
    }

    /// The song was listened to. `own`: this user added it to the room.
    pub fn record(&mut self, song: &Song, own: bool, now: u64) {
        let h = self
            .history
            .songs
            .entry(song.youtube_id.clone())
            .or_default();
        h.score = h.score_at(now) + if own { HEARD_OWN } else { HEARD };
        h.last_unix = now;
        h.plays += 1;
        h.own_plays += own as u32;
        h.title = song.title.clone();
        h.duration_ms = song.duration_ms.or(h.duration_ms);
        if self.history.songs.len() > MAX_HISTORY {
            let mut scores: Vec<f64> = self
                .history
                .songs
                .values()
                .map(|h| h.score_at(now))
                .collect();
            scores.sort_by(f64::total_cmp);
            let cut = scores[scores.len() - MAX_HISTORY * 9 / 10];
            self.history.songs.retain(|_, h| h.score_at(now) >= cut);
        }
    }

    /// Forgets what was listened to (and the recommendations made from it).
    pub fn clear_history(&mut self) {
        self.history = History::default();
    }

    pub fn recs(&self) -> &[Song] {
        &self.history.recs.items
    }

    pub fn set_recs(&mut self, items: Vec<Song>, now: u64) {
        self.history.recs = Recs {
            at_unix: now,
            items,
        };
    }

    pub fn recs_stale(&self, now: u64) -> bool {
        let r = &self.history.recs;
        r.items.is_empty() || r.at_unix > now || now - r.at_unix > RECS_TTL_SECS
    }

    /// Songs to look for similar ones to, with their scores: the three
    /// favourites, and two more drawn at random by score so a refresh brings
    /// something new. Without history or playlists, songs of the room's
    /// queue. `random` yields numbers in 0..1.
    pub fn seeds(
        &self,
        queue: &[shared::Track],
        now: u64,
        mut random: impl FnMut() -> f64,
    ) -> Vec<(Song, f64)> {
        let mut pool: HashMap<&str, (Song, f64)> = HashMap::new();
        for (id, h) in &self.history.songs {
            let song = Song {
                youtube_id: id.clone(),
                title: h.title.clone(),
                duration_ms: h.duration_ms,
                channel: None,
            };
            pool.insert(id, (song, h.score_at(now)));
        }
        let mut saved = HashSet::new();
        for t in self.playlists.iter().flat_map(|p| &p.tracks) {
            if saved.insert(t.youtube_id.as_str()) {
                let e = pool
                    .entry(&t.youtube_id)
                    .or_insert_with(|| (t.clone(), 0.0));
                e.1 += IN_PLAYLIST;
            }
        }
        if pool.is_empty() {
            for t in queue {
                pool.insert(&t.youtube_id, (Song::from(t), 1.0));
            }
        }
        let mut pool: Vec<(Song, f64)> = pool
            .into_values()
            .filter(|(s, w)| shared::youtube::is_valid_video_id(&s.youtube_id) && *w > 0.0)
            .collect();
        // Equal scores (a fresh playlist) come in a random order.
        let mut pool: Vec<(Song, f64, f64)> =
            pool.drain(..).map(|(s, w)| (s, w, random())).collect();
        pool.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.total_cmp(&b.2)));
        let mut rest = pool.split_off(TOP_SEEDS.min(pool.len()));
        let mut out: Vec<(Song, f64)> = pool.into_iter().map(|(s, w, _)| (s, w)).collect();
        for _ in 0..RANDOM_SEEDS {
            let total: f64 = rest.iter().map(|r| r.1).sum();
            if rest.is_empty() || total <= 0.0 {
                break;
            }
            let mut at = random() * total;
            let mut pick = rest.len() - 1;
            for (i, r) in rest.iter().enumerate() {
                at -= r.1;
                if at < 0.0 {
                    pick = i;
                    break;
                }
            }
            let (s, w, _) = rest.swap_remove(pick);
            out.push((s, w));
        }
        out
    }

    /// Orders the songs of the seeds' mixes; one that shows up in several
    /// mixes rises. Leaves out what the user already has or just heard.
    pub fn rank(
        &self,
        seeds: &[(Song, f64)],
        mixes: &[(f64, Vec<Song>)],
        queue: &[shared::Track],
        now: u64,
    ) -> Vec<Song> {
        let mut ids: HashSet<&str> = HashSet::new();
        let mut titles: HashSet<String> = HashSet::new();
        let mut skip = |id, title: &str| {
            ids.insert(id);
            titles.insert(title_key(title));
        };
        for t in queue {
            skip(t.youtube_id.as_str(), &t.title);
        }
        for t in self.playlists.iter().flat_map(|p| &p.tracks) {
            skip(&t.youtube_id, &t.title);
        }
        for (s, _) in seeds {
            skip(&s.youtube_id, &s.title);
        }
        for (id, h) in &self.history.songs {
            if now.saturating_sub(h.last_unix) < RECENT_SECS {
                skip(id, &h.title);
            }
        }
        let mut scored: HashMap<&str, (&Song, f64)> = HashMap::new();
        for (weight, mix) in mixes {
            for (i, s) in mix.iter().enumerate() {
                let length_ok = s
                    .duration_ms
                    .is_none_or(|d| (MIN_REC_MS..=MAX_REC_MS).contains(&d));
                if !length_ok || ids.contains(s.youtube_id.as_str()) {
                    continue;
                }
                scored.entry(&s.youtube_id).or_insert((s, 0.0)).1 += weight / (i as f64 + 3.0);
            }
        }
        let mut list: Vec<(&Song, f64)> = scored.into_values().collect();
        list.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.youtube_id.cmp(&b.0.youtube_id))
        });
        list.into_iter()
            .filter(|(s, _)| titles.insert(title_key(&s.title)))
            .take(MAX_RECS)
            .map(|(s, _)| s.clone())
            .collect()
    }

    pub fn save_playlists(&self) -> Result<(), String> {
        write(self.playlists_file.as_deref(), &self.playlists)
    }

    pub fn save_history(&self) -> Result<(), String> {
        write(self.history_file.as_deref(), &self.history)
    }

    fn get_mut(&mut self, id: Uuid) -> Option<&mut Playlist> {
        self.playlists.iter_mut().find(|p| p.id == id)
    }
}

/// Reads a JSON file. Unreadable content is set aside as `.bad`, so it is
/// never overwritten; a file that cannot be set aside is not saved to.
fn read<T: DeserializeOwned + Default>(path: PathBuf) -> (T, Option<PathBuf>) {
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (T::default(), Some(path)),
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            return (T::default(), None);
        }
    };
    // Tolerate a BOM from hand-edited files.
    let json = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    match serde_json::from_slice(json) {
        Ok(v) => (v, Some(path)),
        Err(e) => {
            tracing::warn!("{} is damaged: {e}", path.display());
            let mut bad = path.with_extension("json.bad");
            if bad.exists() {
                bad = path.with_extension(format!("json.{}.bad", unix_now()));
            }
            match std::fs::rename(&path, &bad) {
                Ok(()) => (T::default(), Some(path)),
                Err(e) => {
                    tracing::warn!("cannot set it aside: {e}");
                    (T::default(), None)
                }
            }
        }
    }
}

fn write<T: Serialize>(path: Option<&Path>, value: &T) -> Result<(), String> {
    let Some(path) = path else { return Ok(()) };
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// "Song (Official Video) [4K]" and "song" are the same song.
fn title_key(title: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let key = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if key.is_empty() {
        title.to_lowercase()
    } else {
        key
    }
}

/// Splits songs into batches that fit in one message to the server.
pub fn batches(songs: &[Song]) -> Vec<Vec<TrackInfo>> {
    let mut out: Vec<Vec<TrackInfo>> = Vec::new();
    let mut size = 0;
    for s in songs {
        let t = TrackInfo {
            youtube_id: s.youtube_id.clone(),
            title: s.title.clone(),
            duration_ms: s.duration_ms,
        };
        let len = serde_json::to_string(&t).map_or(0, |j| j.len() + 1);
        match out.last_mut() {
            Some(batch) if size + len <= BATCH_BYTES => {
                size += len;
                batch.push(t);
            }
            _ => {
                size = len;
                out.push(vec![t]);
            }
        }
    }
    out
}

/// Decides when a playing song counts as listened to.
#[derive(Debug, Default)]
pub struct ListenTimer {
    track: Option<Uuid>,
    heard: Duration,
    counted: bool,
    last: Option<Instant>,
}

impl ListenTimer {
    /// Call often. True once per play, when enough was heard.
    pub fn tick(
        &mut self,
        track: Option<Uuid>,
        playing: bool,
        duration_ms: Option<u64>,
        now: Instant,
    ) -> bool {
        if track != self.track {
            *self = Self {
                track,
                ..Self::default()
            };
        }
        let last = self.last.take();
        if track.is_none() || !playing {
            return false;
        }
        self.last = Some(now);
        if let Some(last) = last {
            self.heard += now.saturating_duration_since(last).min(LISTEN_STEP);
        }
        let need = duration_ms.map_or(LISTEN_MS, |d| LISTEN_MS.min(d / 2));
        if self.counted || self.heard < Duration::from_millis(need) {
            return false;
        }
        self.counted = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 3600;
    const NOW: u64 = 1_800_000_000;

    fn id(n: usize) -> String {
        format!("video{n:06}")
    }

    fn song(n: usize) -> Song {
        Song {
            youtube_id: id(n),
            title: format!("Pesma {n}"),
            duration_ms: Some(200_000),
            channel: None,
        }
    }

    fn track(n: usize) -> shared::Track {
        shared::Track {
            id: Uuid::new_v4(),
            youtube_id: id(n),
            title: format!("Pesma {n}"),
            duration_ms: Some(200_000),
            added_by: "Ana".into(),
        }
    }

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("synctube-lib-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scores_own_songs_higher_and_fade() {
        let mut l = Library::default();
        l.record(&song(1), false, NOW);
        l.record(&song(2), true, NOW);
        let at = |l: &Library, n, t| l.history.songs[&id(n)].score_at(t);
        assert_eq!(at(&l, 1, NOW), 1.0);
        assert_eq!(at(&l, 2, NOW), 2.5);
        assert!((at(&l, 2, NOW + 14 * DAY) - 1.25).abs() < 1e-9);
        l.record(&song(2), false, NOW + 14 * DAY);
        assert!((at(&l, 2, NOW + 14 * DAY) - 2.25).abs() < 1e-9);
        assert_eq!(l.history.songs[&id(2)].plays, 2);
        assert_eq!(l.history.songs[&id(2)].own_plays, 1);
    }

    #[test]
    fn history_is_capped_and_keeps_favourites() {
        let mut l = Library::default();
        l.record(&song(0), true, NOW);
        for n in 1..=MAX_HISTORY {
            l.record(&song(n), n % 2 == 0, NOW + n as u64);
        }
        assert!(l.history.songs.len() <= MAX_HISTORY);
        assert!(l.history.songs.contains_key(&id(MAX_HISTORY)));
    }

    #[test]
    fn seeds_take_favourites_and_random_rest() {
        let mut l = Library::default();
        for n in 1..=10 {
            for _ in 0..n {
                l.record(&song(n), false, NOW);
            }
        }
        let s = l.seeds(&[], NOW, || 0.0);
        let ids: Vec<_> = s.iter().map(|(s, _)| s.youtube_id.clone()).collect();
        assert_eq!(ids[..3], [id(10), id(9), id(8)]);
        assert_eq!(ids.len(), 5);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 5);
        assert_eq!(l.seeds(&[], NOW, || 0.999_999).len(), 5);
    }

    #[test]
    fn seeds_count_playlists_and_fall_back_to_the_queue() {
        let mut l = Library::default();
        assert!(l.seeds(&[], NOW, || 0.5).is_empty());
        assert_eq!(l.seeds(&[track(1), track(2)], NOW, || 0.5).len(), 2);
        l.record(&song(1), false, NOW);
        let p = l.create("Moja").unwrap();
        l.add(p, [song(2)]);
        // With a taste of one's own the queue no longer matters.
        let s = l.seeds(&[track(3)], NOW, || 0.5);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0.youtube_id, id(2), "in a playlist beats heard once");
    }

    #[test]
    fn rank_prefers_shared_songs_and_filters() {
        let mut l = Library::default();
        l.record(&song(50), false, NOW - DAY);
        l.record(&song(51), false, NOW - 8 * DAY);
        let p = l.create("Moja").unwrap();
        l.add(p, [song(60)]);
        let short = Song {
            duration_ms: Some(30_000),
            ..song(70)
        };
        let long = Song {
            duration_ms: Some(3_600_000),
            ..song(71)
        };
        let twin = Song {
            title: "PESMA 2 (Official Video) [4K]".into(),
            ..song(72)
        };
        let seeds = [(song(1), 2.0), (song(9), 2.0)];
        let mixes = [
            (
                2.0,
                vec![
                    song(1),
                    song(2),
                    song(3),
                    song(50),
                    song(51),
                    song(60),
                    short,
                    long,
                ],
            ),
            (2.0, vec![song(9), song(4), song(3), song(80), twin]),
        ];
        let ids: Vec<_> = l
            .rank(&seeds, &mixes, &[track(80)], NOW)
            .into_iter()
            .map(|s| s.youtube_id)
            .collect();
        assert_eq!(ids[0], id(3), "in both mixes");
        assert_eq!(
            ids.iter().cloned().collect::<HashSet<_>>(),
            HashSet::from([id(2), id(3), id(4), id(51)])
        );
    }

    #[test]
    fn rank_is_capped() {
        let l = Library::default();
        let mix = (100..200).map(song).collect();
        assert_eq!(l.rank(&[], &[(1.0, mix)], &[], NOW).len(), MAX_RECS);
    }

    #[test]
    fn recs_go_stale() {
        let mut l = Library::default();
        assert!(l.recs_stale(NOW));
        l.set_recs(vec![song(1)], NOW);
        assert!(!l.recs_stale(NOW + 3600));
        assert!(l.recs_stale(NOW + 13 * 3600));
        assert!(l.recs_stale(NOW - 10), "the clock went back");
    }

    #[test]
    fn playlists_skip_duplicates_and_cap() {
        let mut l = Library::default();
        assert_eq!(l.create("   "), None);
        let p = l.create(" Žurka ").unwrap();
        assert_eq!(l.create("žurka"), Some(p), "same name, same playlist");
        let first = l.add(p, [song(1), song(2), song(1)]).unwrap();
        assert_eq!((first.added, first.no_room), (2, 0));
        assert_eq!(l.add(p, [song(2)]).unwrap().added, 0);
        let bad = Song {
            youtube_id: "x".into(),
            ..song(1)
        };
        assert_eq!(l.add(p, [bad]).unwrap().added, 0);
        let many = l.add(p, (3..600).map(song)).unwrap();
        assert_eq!(l.get(p).unwrap().tracks.len(), MAX_PLAYLIST);
        assert_eq!(many.added + many.no_room, 597);
        assert_eq!(l.add(Uuid::new_v4(), [song(1)]), None);
    }

    #[test]
    fn playlists_edit() {
        let mut l = Library::default();
        let p = l.create("A").unwrap();
        l.add(p, (1..=3).map(song));
        l.move_song(p, &id(3), 0);
        l.move_song(p, &id(1), 99);
        l.remove(p, &id(2));
        let ids: Vec<_> = l
            .get(p)
            .unwrap()
            .tracks
            .iter()
            .map(|t| &t.youtube_id)
            .collect();
        assert_eq!(ids, [&id(3), &id(1)]);
        l.rename(p, "B");
        l.rename(p, " ");
        assert_eq!(l.get(p).unwrap().name, "B");
        l.delete(p);
        assert!(l.playlists().is_empty());
    }

    #[test]
    fn learns_durations() {
        let mut l = Library::default();
        let p = l.create("A").unwrap();
        l.add(
            p,
            [Song {
                duration_ms: None,
                ..song(1)
            }],
        );
        assert!(!l.learn_durations(&[track(2)]));
        assert!(l.learn_durations(&[track(1)]));
        assert_eq!(l.get(p).unwrap().tracks[0].duration_ms, Some(200_000));
        assert!(!l.learn_durations(&[track(1)]));
    }

    #[test]
    fn survives_a_restart() {
        let dir = temp_dir();
        let mut l = Library::load(Some(&dir));
        let p = l.create("Moja").unwrap();
        l.add(p, [song(1)]);
        l.record(&song(2), true, NOW);
        l.set_recs(vec![song(3)], NOW);
        l.save_playlists().unwrap();
        l.save_history().unwrap();
        let mut back = Library::load(Some(&dir));
        assert_eq!(back.playlists, l.playlists);
        assert_eq!(back.history, l.history);
        back.clear_history();
        back.save_history().unwrap();
        let back = Library::load(Some(&dir));
        assert!(!back.has_history());
        assert!(back.recs().is_empty());
        assert_eq!(back.playlists, l.playlists, "playlists stay");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_file_is_set_aside() {
        let dir = temp_dir();
        let file = dir.join("playlists.json");
        for text in ["{not json", "[[["] {
            std::fs::write(&file, text).unwrap();
            let mut l = Library::load(Some(&dir));
            assert!(l.playlists().is_empty());
            l.create("Nova");
            l.save_playlists().unwrap();
        }
        let mut bad: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "bad"))
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        bad.sort();
        assert_eq!(bad, ["[[[", "{not json"]);
        assert_eq!(Library::load(Some(&dir)).playlists().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn batches_fit_in_a_message() {
        let songs: Vec<Song> = (0..500)
            .map(|n| Song {
                title: "ž\"".repeat(100),
                ..song(n)
            })
            .collect();
        let b = batches(&songs);
        assert!(b.len() > 1);
        for batch in &b {
            let msg = shared::ClientMsg::AddResolved {
                request_id: u64::MAX,
                tracks: batch.clone(),
            };
            let len = serde_json::to_string(&msg).unwrap().len();
            assert!(len < shared::limits::MAX_MESSAGE_BYTES);
        }
        let order: Vec<String> = b.into_iter().flatten().map(|t| t.youtube_id).collect();
        assert_eq!(order, (0..500).map(id).collect::<Vec<_>>());
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn title_key_ignores_brackets() {
        assert_eq!(title_key("Ana  (Official Video) [4K]"), "ana");
        assert_eq!(title_key("(Intro)"), "(intro)");
    }

    #[test]
    fn listening_counts_once_after_thirty_seconds() {
        let mut t = ListenTimer::default();
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let a = Some(Uuid::new_v4());
        let hits: Vec<u64> = (0..100)
            .filter(|&s| t.tick(a, true, Some(200_000), at(s)))
            .collect();
        assert_eq!(hits, [30]);
        // A new song starts over; a short one needs half of it.
        let b = Some(Uuid::new_v4());
        let hits: Vec<u64> = (100..140)
            .filter(|&s| t.tick(b, true, Some(20_000), at(s)))
            .collect();
        assert_eq!(hits, [110]);
    }

    #[test]
    fn pauses_and_gaps_do_not_count() {
        let mut t = ListenTimer::default();
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let a = Some(Uuid::new_v4());
        t.tick(a, true, None, at(0));
        // The computer slept for an hour.
        assert!(!t.tick(a, true, None, at(3600)));
        for s in 0..60 {
            assert!(!t.tick(a, false, None, at(3601 + s)));
        }
        assert!(!t.tick(a, true, None, at(3700)));
        assert_eq!(t.heard, LISTEN_STEP);
    }
}
