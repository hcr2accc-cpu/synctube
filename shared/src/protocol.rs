//! Wire protocol: JSON over WebSocket, tagged with `"type"`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A track in the shared queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: Uuid,
    pub youtube_id: String,
    pub title: String,
    pub duration_ms: Option<u64>,
    pub added_by: String,
}

/// Metadata of a track as resolved by a client (before it gets an id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub youtube_id: String,
    pub title: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Playback timeline. The server never "ticks" a position; it stores an
/// anchor and every client derives the position from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    pub track_index: Option<usize>,
    pub status: PlayStatus,
    /// Server time (ms) at which the track was (or will be) at `anchor_position_ms`.
    pub anchor_server_ms: u64,
    pub anchor_position_ms: u64,
    /// Incremented on every playback change (track, play, pause, seek).
    pub version: u64,
}

impl PlaybackState {
    /// Expected track position at the given server time.
    pub fn position_at(&self, server_now_ms: u64) -> u64 {
        crate::sync::expected_position_ms(self, server_now_ms)
    }
}

/// A connected room member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: Uuid,
    pub name: String,
}

/// Full room snapshot (small enough to send on every change).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoomState {
    pub name: String,
    pub queue: Vec<Track>,
    pub playback: PlaybackState,
    /// Incremented on every change of the room state (queue or playback).
    pub revision: u64,
    /// True while the server waits for clients to report `Ready` before starting.
    #[serde(default)]
    pub waiting_for_ready: bool,
}

impl RoomState {
    /// The track the playback state points to, if any.
    pub fn current_track(&self) -> Option<&Track> {
        self.playback.track_index.and_then(|i| self.queue.get(i))
    }
}

/// Messages sent by clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    Join {
        room: String,
        #[serde(default)]
        password: Option<String>,
        name: String,
    },
    /// Ask to add a YouTube URL. The server validates it and answers the same
    /// client with `ResolveRequest`; the client resolves metadata locally.
    Add {
        url: String,
    },
    AddResolved {
        request_id: u64,
        tracks: Vec<TrackInfo>,
    },
    /// Fills in the duration of queued tracks that were added without one
    /// (quick adds use a fast title lookup and learn the duration later).
    Duration {
        youtube_id: String,
        duration_ms: u64,
    },
    Play,
    Pause,
    Seek {
        position_ms: u64,
    },
    /// `version` is the playback version the command was based on; stale
    /// commands are ignored so a simultaneous double-skip skips once.
    Next {
        #[serde(default)]
        version: Option<u64>,
    },
    Previous {
        #[serde(default)]
        version: Option<u64>,
    },
    /// Jump directly to a track in the queue.
    PlayTrack {
        track_id: Uuid,
    },
    Remove {
        track_id: Uuid,
    },
    Move {
        track_id: Uuid,
        new_index: usize,
    },
    /// The client has the current track loaded and positioned for `version`.
    Ready {
        version: u64,
    },
    /// The client reached the end of the track (or cannot play it).
    Ended {
        version: u64,
    },
    Ping {
        client_ms: u64,
    },
    Leave,
}

/// Messages sent by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    Welcome {
        you: Member,
        room_state: RoomState,
        server_ms: u64,
    },
    State {
        room_state: RoomState,
    },
    ResolveRequest {
        request_id: u64,
        url: String,
    },
    Pong {
        client_ms: u64,
        server_ms: u64,
    },
    Members {
        list: Vec<Member>,
    },
    Error {
        message: String,
    },
    /// Join was refused (e.g. wrong password); the connection will close and
    /// the client should not auto-reconnect.
    Rejected {
        message: String,
    },
    Toast {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_json_shape() {
        let m = ClientMsg::Seek { position_ms: 1500 };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"type":"Seek","position_ms":1500}"#);
        let back: ClientMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn next_without_version_parses() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"Next"}"#).unwrap();
        assert_eq!(m, ClientMsg::Next { version: None });
        let m: ClientMsg = serde_json::from_str(r#"{"type":"Play"}"#).unwrap();
        assert_eq!(m, ClientMsg::Play);
    }

    #[test]
    fn server_msg_roundtrip() {
        let st = RoomState {
            name: "ekipa".into(),
            queue: vec![Track {
                id: Uuid::new_v4(),
                youtube_id: "dQw4w9WgXcQ".into(),
                title: "Pesma".into(),
                duration_ms: Some(1000),
                added_by: "Ana".into(),
            }],
            playback: PlaybackState {
                track_index: Some(0),
                status: PlayStatus::Playing,
                anchor_server_ms: 10,
                anchor_position_ms: 20,
                version: 3,
            },
            revision: 7,
            waiting_for_ready: false,
        };
        let m = ServerMsg::State { room_state: st };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.starts_with(r#"{"type":"State""#));
        let back: ServerMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }
}
