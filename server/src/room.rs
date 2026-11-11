//! One tokio task per room. All state mutations happen here, so there are no
//! lock races between connections.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shared::limits::{clamp_chars, MAX_QUEUE, MAX_TITLE_CHARS};
use shared::youtube::{is_valid_video_id, parse_youtube_url, playlist_url, watch_url, YoutubeRef};
use shared::{ClientMsg, Member, PlayStatus, PlaybackState, RoomState, ServerMsg, Track};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::conn::Out;
use crate::{now_ms, strings, AppState};

/// Longest track duration accepted from clients (24 h).
const MAX_DURATION_MS: u64 = 24 * 60 * 60 * 1000;
/// Pending resolve requests expire after this long.
const RESOLVE_TTL: Duration = Duration::from_secs(180);
/// Max outstanding resolve requests per connection.
const MAX_PENDING_PER_CONN: usize = 5;
/// "Previous" restarts the current track when it played longer than this.
const PREVIOUS_RESTART_MS: u64 = 3000;

pub(crate) enum RoomCmd {
    Join {
        conn: Uuid,
        name: String,
        password: Option<String>,
        out: mpsc::Sender<Out>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Msg {
        conn: Uuid,
        msg: ClientMsg,
    },
    Leave {
        conn: Uuid,
    },
}

struct MemberConn {
    id: Uuid,
    name: String,
    out: mpsc::Sender<Out>,
}

struct Barrier {
    version: u64,
    deadline: Instant,
    ready: HashSet<Uuid>,
    /// Members present when loading began. Someone who joins later does not
    /// hold the others back; they catch up on their own once playback runs.
    waiting_for: HashSet<Uuid>,
}

struct Room {
    name: String,
    app: AppState,
    password: Option<String>,
    created: bool,
    members: Vec<MemberConn>,
    queue: Vec<Track>,
    playback: PlaybackState,
    revision: u64,
    barrier: Option<Barrier>,
    pending: HashMap<u64, (Uuid, Instant)>,
    next_request_id: u64,
    empty_since: Option<Instant>,
    dirty: bool,
}

pub(crate) async fn run_room(
    name: String,
    generation: u64,
    mut rx: mpsc::Receiver<RoomCmd>,
    app: AppState,
) {
    let mut room = Room {
        name: name.clone(),
        app: app.clone(),
        password: None,
        created: false,
        members: Vec::new(),
        queue: Vec::new(),
        playback: PlaybackState::default(),
        revision: 0,
        barrier: None,
        pending: HashMap::new(),
        next_request_id: 1,
        empty_since: Some(Instant::now()),
        dirty: false,
    };
    loop {
        let deadline = room.next_deadline();
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(cmd) => room.handle(cmd),
                None => break,
            },
            _ = sleep_until_opt(deadline) => room.on_timer(),
        }
        room.flush();
        if room.expired() {
            app.remove_room(&name, generation);
            break;
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d.into()).await,
        None => std::future::pending().await,
    }
}

impl Room {
    fn cfg(&self) -> &crate::Config {
        self.app.config()
    }

    fn snapshot(&self) -> RoomState {
        RoomState {
            name: self.name.clone(),
            queue: self.queue.clone(),
            playback: self.playback.clone(),
            revision: self.revision,
            waiting_for_ready: self.barrier.is_some(),
        }
    }

    fn members_list(&self) -> Vec<Member> {
        self.members
            .iter()
            .map(|m| Member {
                id: m.id,
                name: m.name.clone(),
            })
            .collect()
    }

    fn send_to(&self, conn: Uuid, msg: &ServerMsg) {
        if let Some(m) = self.members.iter().find(|m| m.id == conn) {
            send(&m.out, msg);
        }
    }

    fn broadcast(&self, msg: &ServerMsg) {
        let text: Arc<str> = match serde_json::to_string(msg) {
            Ok(s) => s.into(),
            Err(_) => return,
        };
        for m in &self.members {
            if m.out.try_send(Out::Text(text.clone())).is_err() {
                tracing::warn!(member = %m.name, "outgoing queue full, dropping message");
            }
        }
    }

    fn broadcast_except(&self, conn: Uuid, msg: &ServerMsg) {
        for m in self.members.iter().filter(|m| m.id != conn) {
            send(&m.out, msg);
        }
    }

    fn toast_all(&self, message: String) {
        self.broadcast(&ServerMsg::Toast { message });
    }

    fn error(&self, conn: Uuid, message: &str) {
        self.send_to(
            conn,
            &ServerMsg::Error {
                message: message.to_string(),
            },
        );
    }

    /// Marks the state as changed; it is broadcast once after the command.
    fn touch(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }

    fn flush(&mut self) {
        if self.dirty {
            self.dirty = false;
            let msg = ServerMsg::State {
                room_state: self.snapshot(),
            };
            self.broadcast(&msg);
        }
    }

    fn expired(&self) -> bool {
        self.members.is_empty()
            && self
                .empty_since
                .is_some_and(|t| t.elapsed() >= self.cfg().empty_room_ttl)
    }

    fn current_duration(&self) -> Option<u64> {
        self.playback
            .track_index
            .and_then(|i| self.queue.get(i))
            .and_then(|t| t.duration_ms)
    }

    fn current_position(&self, now: u64) -> u64 {
        shared::sync::expected_position_clamped(&self.playback, now, self.current_duration())
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut deadline: Option<Instant> = None;
        let mut consider = |d: Instant| {
            deadline = Some(deadline.map_or(d, |cur| cur.min(d)));
        };
        if let Some(b) = &self.barrier {
            consider(b.deadline);
        }
        if let Some(end_ms) = self.auto_advance_at_ms() {
            let now = now_ms();
            consider(Instant::now() + Duration::from_millis(end_ms.saturating_sub(now)));
        }
        if self.members.is_empty() {
            if let Some(t) = self.empty_since {
                consider(t + self.cfg().empty_room_ttl);
            }
        }
        deadline
    }

    /// Server time at which the current track should be over (plus grace).
    fn auto_advance_at_ms(&self) -> Option<u64> {
        if self.playback.status != PlayStatus::Playing {
            return None;
        }
        let dur = self.current_duration()?;
        Some(
            self.playback.anchor_server_ms
                + dur.saturating_sub(self.playback.anchor_position_ms)
                + self.cfg().end_grace_ms,
        )
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        if self.barrier.as_ref().is_some_and(|b| now >= b.deadline) {
            tracing::debug!(room = %self.name, "ready barrier timed out");
            self.go_playing(self.cfg().start_lead_ms);
        }
        if self.auto_advance_at_ms().is_some_and(|t| now_ms() >= t) {
            tracing::debug!(room = %self.name, "auto-advance on server timeline");
            self.advance();
        }
    }

    // ----- playback transitions -------------------------------------------------

    /// Loads track `idx` paused at `position_ms` and waits for `Ready`.
    fn begin_loading(&mut self, idx: usize, position_ms: u64, timeout: Duration) {
        let version = self.playback.version + 1;
        self.playback = PlaybackState {
            track_index: Some(idx),
            status: PlayStatus::Paused,
            anchor_server_ms: now_ms(),
            anchor_position_ms: position_ms,
            version,
        };
        self.barrier = Some(Barrier {
            version,
            deadline: Instant::now() + timeout,
            ready: HashSet::new(),
            waiting_for: self.members.iter().map(|m| m.id).collect(),
        });
        self.touch();
    }

    fn start_track(&mut self, idx: usize) {
        let timeout = self.cfg().barrier_timeout;
        self.begin_loading(idx, 0, timeout);
    }

    fn go_playing(&mut self, lead_ms: u64) {
        if self.playback.track_index.is_none() {
            return;
        }
        self.barrier = None;
        self.playback.status = PlayStatus::Playing;
        self.playback.anchor_server_ms = now_ms() + lead_ms;
        self.playback.version += 1;
        self.touch();
    }

    fn pause(&mut self) {
        let now = now_ms();
        match self.playback.status {
            PlayStatus::Playing => {
                self.playback.anchor_position_ms = self.current_position(now);
            }
            PlayStatus::Paused if self.barrier.is_some() => {}
            _ => return,
        }
        self.barrier = None;
        self.playback.status = PlayStatus::Paused;
        self.playback.anchor_server_ms = now;
        self.playback.version += 1;
        self.touch();
    }

    fn stop(&mut self) {
        self.barrier = None;
        self.playback = PlaybackState {
            track_index: None,
            status: PlayStatus::Stopped,
            anchor_server_ms: now_ms(),
            anchor_position_ms: 0,
            version: self.playback.version + 1,
        };
        self.touch();
    }

    fn advance(&mut self) {
        match self.playback.track_index {
            Some(i) if i + 1 < self.queue.len() => self.start_track(i + 1),
            _ => self.stop(),
        }
    }

    fn check_barrier(&mut self) {
        let done = match &self.barrier {
            Some(b) => {
                !self.members.is_empty()
                    && self
                        .members
                        .iter()
                        .filter(|m| b.waiting_for.contains(&m.id))
                        .all(|m| b.ready.contains(&m.id))
            }
            None => false,
        };
        if done {
            self.go_playing(self.cfg().start_lead_ms);
        }
    }

    fn is_stale(&self, version: Option<u64>) -> bool {
        version.is_some_and(|v| v != self.playback.version)
    }

    // ----- commands -------------------------------------------------------------

    fn handle(&mut self, cmd: RoomCmd) {
        match cmd {
            RoomCmd::Join {
                conn,
                name,
                password,
                out,
                reply,
            } => self.join(conn, name, password, out, reply),
            RoomCmd::Leave { conn } => self.leave(conn),
            RoomCmd::Msg { conn, msg } => {
                if self.members.iter().any(|m| m.id == conn) {
                    self.handle_msg(conn, msg);
                }
            }
        }
    }

    fn join(
        &mut self,
        conn: Uuid,
        name: String,
        password: Option<String>,
        out: mpsc::Sender<Out>,
        reply: oneshot::Sender<Result<(), String>>,
    ) {
        let password = password.filter(|p| !p.is_empty());
        if !self.created {
            self.created = true;
            self.password = password.clone();
        }
        if self.password.is_some() && self.password != password {
            let _ = reply.send(Err(strings::WRONG_PASSWORD.to_string()));
            return;
        }
        if self.members.len() >= self.cfg().max_members {
            let _ = reply.send(Err(strings::ROOM_FULL.to_string()));
            return;
        }
        if reply.send(Ok(())).is_err() {
            return;
        }
        let me = Member {
            id: conn,
            name: name.clone(),
        };
        self.members.push(MemberConn {
            id: conn,
            name: name.clone(),
            out,
        });
        self.empty_since = None;
        tracing::info!(room = %self.name, %name, members = self.members.len(), "member joined");
        self.send_to(
            conn,
            &ServerMsg::Welcome {
                you: me,
                room_state: self.snapshot(),
                server_ms: now_ms(),
            },
        );
        self.broadcast(&ServerMsg::Members {
            list: self.members_list(),
        });
        self.broadcast_except(
            conn,
            &ServerMsg::Toast {
                message: strings::joined(&name),
            },
        );
    }

    fn leave(&mut self, conn: Uuid) {
        let Some(pos) = self.members.iter().position(|m| m.id == conn) else {
            return;
        };
        let m = self.members.remove(pos);
        self.pending.retain(|_, (c, _)| *c != conn);
        tracing::info!(room = %self.name, name = %m.name, members = self.members.len(), "member left");
        if self.members.is_empty() {
            self.empty_since = Some(Instant::now());
            // Nobody listens: hold the position instead of running the queue.
            self.pause();
            return;
        }
        if let Some(b) = &mut self.barrier {
            b.ready.remove(&conn);
            b.waiting_for.remove(&conn);
        }
        self.broadcast(&ServerMsg::Members {
            list: self.members_list(),
        });
        self.toast_all(strings::left(&m.name));
        self.check_barrier();
    }

    fn member_name(&self, conn: Uuid) -> String {
        self.members
            .iter()
            .find(|m| m.id == conn)
            .map(|m| m.name.clone())
            .unwrap_or_default()
    }

    fn handle_msg(&mut self, conn: Uuid, msg: ClientMsg) {
        let now = now_ms();
        match msg {
            ClientMsg::Join { .. } => self.error(conn, strings::ALREADY_JOINED),
            ClientMsg::Ping { .. } | ClientMsg::Leave => {}
            ClientMsg::Add { url } => self.add(conn, &url),
            ClientMsg::AddResolved { request_id, tracks } => {
                self.add_resolved(conn, request_id, tracks)
            }
            ClientMsg::Duration {
                youtube_id,
                duration_ms,
            } => self.set_duration(&youtube_id, duration_ms),
            ClientMsg::Play => match self.playback.status {
                PlayStatus::Paused if self.barrier.is_none() => {
                    if self.playback.track_index.is_some() {
                        self.go_playing(self.cfg().resume_lead_ms);
                    }
                }
                PlayStatus::Stopped if !self.queue.is_empty() => self.start_track(0),
                _ => {}
            },
            ClientMsg::Pause => self.pause(),
            ClientMsg::Seek { position_ms } => {
                if self.playback.track_index.is_none() {
                    return;
                }
                let pos = match self.current_duration() {
                    Some(d) => position_ms.min(d),
                    None => position_ms,
                };
                match self.playback.status {
                    PlayStatus::Playing => {
                        let idx = self.playback.track_index.unwrap_or(0);
                        let timeout = self.cfg().seek_barrier_timeout;
                        self.begin_loading(idx, pos, timeout);
                    }
                    PlayStatus::Paused => {
                        self.playback.anchor_position_ms = pos;
                        self.playback.anchor_server_ms = now;
                        self.playback.version += 1;
                        if let Some(b) = &mut self.barrier {
                            b.version = self.playback.version;
                            b.ready.clear();
                        }
                        self.touch();
                    }
                    PlayStatus::Stopped => {}
                }
            }
            ClientMsg::Next { version } => {
                if !self.is_stale(version) && self.playback.track_index.is_some() {
                    self.advance();
                }
            }
            ClientMsg::Previous { version } => {
                if self.is_stale(version) {
                    return;
                }
                match self.playback.track_index {
                    Some(i) => {
                        if self.current_position(now) > PREVIOUS_RESTART_MS || i == 0 {
                            self.start_track(i);
                        } else {
                            self.start_track(i - 1);
                        }
                    }
                    None if !self.queue.is_empty() => self.start_track(self.queue.len() - 1),
                    None => {}
                }
            }
            ClientMsg::PlayTrack { track_id } => {
                if let Some(idx) = self.queue.iter().position(|t| t.id == track_id) {
                    self.start_track(idx);
                }
            }
            ClientMsg::Remove { track_id } => self.remove(track_id),
            ClientMsg::Move {
                track_id,
                new_index,
            } => self.move_track(track_id, new_index),
            ClientMsg::Ready { version } => {
                if let Some(b) = &mut self.barrier {
                    if b.version == version {
                        b.ready.insert(conn);
                    }
                }
                self.check_barrier();
            }
            ClientMsg::Ended { version } => {
                if version == self.playback.version
                    && self.playback.track_index.is_some()
                    && self.playback.status != PlayStatus::Stopped
                {
                    self.advance();
                }
            }
        }
    }

    fn add(&mut self, conn: Uuid, url: &str) {
        let canonical = match parse_youtube_url(url) {
            Some(YoutubeRef::Video(id)) => watch_url(&id),
            Some(YoutubeRef::Playlist(id)) => playlist_url(&id),
            None => return self.error(conn, strings::NOT_YOUTUBE),
        };
        if self.queue.len() >= MAX_QUEUE {
            return self.error(conn, strings::QUEUE_FULL);
        }
        self.pending.retain(|_, (_, t)| t.elapsed() < RESOLVE_TTL);
        if self.pending.values().filter(|(c, _)| *c == conn).count() >= MAX_PENDING_PER_CONN {
            return self.error(conn, strings::TOO_MANY_PENDING);
        }
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        self.pending.insert(request_id, (conn, Instant::now()));
        self.send_to(
            conn,
            &ServerMsg::ResolveRequest {
                request_id,
                url: canonical,
            },
        );
    }

    fn add_resolved(&mut self, conn: Uuid, request_id: u64, tracks: Vec<shared::TrackInfo>) {
        match self.pending.get(&request_id) {
            Some((c, t)) if *c == conn && t.elapsed() < RESOLVE_TTL => {
                self.pending.remove(&request_id);
            }
            _ => return self.error(conn, strings::UNKNOWN_REQUEST),
        }
        let added_by = self.member_name(conn);
        let room_left = MAX_QUEUE.saturating_sub(self.queue.len());
        let valid: Vec<Track> = tracks
            .into_iter()
            .filter(|t| is_valid_video_id(&t.youtube_id))
            .map(|t| {
                let title = clamp_chars(&t.title, MAX_TITLE_CHARS);
                Track {
                    id: Uuid::new_v4(),
                    title: if title.is_empty() {
                        t.youtube_id.clone()
                    } else {
                        title
                    },
                    youtube_id: t.youtube_id,
                    duration_ms: t.duration_ms.filter(|d| *d > 0 && *d <= MAX_DURATION_MS),
                    added_by: added_by.clone(),
                }
            })
            .collect();
        let total_valid = valid.len();
        let new: Vec<Track> = valid.into_iter().take(room_left).collect();
        if new.is_empty() {
            let msg = if total_valid > 0 {
                strings::QUEUE_FULL
            } else {
                strings::NOTHING_ADDED
            };
            return self.error(conn, msg);
        }
        let first_new = self.queue.len();
        let count = new.len();
        let first_title = new[0].title.clone();
        self.queue.extend(new);
        self.touch();
        if count < total_valid {
            self.error(conn, &strings::truncated(count));
        }
        self.toast_all(if count == 1 {
            strings::added_one(&added_by, &first_title)
        } else {
            strings::added_many(&added_by, count)
        });
        if self.playback.status == PlayStatus::Stopped {
            self.start_track(first_new);
        }
    }

    /// Sets the duration of queued tracks that don't have one yet.
    fn set_duration(&mut self, youtube_id: &str, duration_ms: u64) {
        if duration_ms == 0 || duration_ms > MAX_DURATION_MS {
            return;
        }
        let mut changed = false;
        for t in self
            .queue
            .iter_mut()
            .filter(|t| t.youtube_id == youtube_id && t.duration_ms.is_none())
        {
            t.duration_ms = Some(duration_ms);
            changed = true;
        }
        if changed {
            self.touch();
        }
    }

    fn remove(&mut self, track_id: Uuid) {
        let Some(idx) = self.queue.iter().position(|t| t.id == track_id) else {
            return;
        };
        self.queue.remove(idx);
        self.touch();
        match self.playback.track_index {
            Some(cur) if idx < cur => self.playback.track_index = Some(cur - 1),
            Some(cur) if idx == cur => {
                if idx < self.queue.len() {
                    self.start_track(idx);
                } else {
                    self.stop();
                }
            }
            _ => {}
        }
    }

    fn move_track(&mut self, track_id: Uuid, new_index: usize) {
        let Some(idx) = self.queue.iter().position(|t| t.id == track_id) else {
            return;
        };
        let new_index = new_index.min(self.queue.len() - 1);
        if idx == new_index {
            return;
        }
        let current_id = self
            .playback
            .track_index
            .and_then(|i| self.queue.get(i))
            .map(|t| t.id);
        let t = self.queue.remove(idx);
        self.queue.insert(new_index, t);
        if let Some(cid) = current_id {
            self.playback.track_index = self.queue.iter().position(|t| t.id == cid);
        }
        self.touch();
    }
}

fn send(out: &mpsc::Sender<Out>, msg: &ServerMsg) {
    if let Ok(s) = serde_json::to_string(msg) {
        let _ = out.try_send(Out::Text(s.into()));
    }
}
