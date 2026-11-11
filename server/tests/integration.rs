//! Spins up the real server and drives it with fake WebSocket clients.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use server::Config;
use shared::{ClientMsg, Member, PlayStatus, RoomState, ServerMsg, TrackInfo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const T: Duration = Duration::from_secs(3);

fn fast_cfg() -> Config {
    Config {
        barrier_timeout: Duration::from_millis(800),
        seek_barrier_timeout: Duration::from_millis(600),
        start_lead_ms: 100,
        resume_lead_ms: 50,
        end_grace_ms: 200,
        empty_room_ttl: Duration::from_secs(2),
        ..Config::default()
    }
}

async fn start(cfg: Config) -> (String, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(server::serve(listener, cfg));
    (format!("ws://{addr}/ws"), addr)
}

struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    state: RoomState,
    members: Vec<Member>,
    you: Member,
}

impl Client {
    async fn connect_raw(url: &str) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    async fn join(
        url: &str,
        room: &str,
        name: &str,
        password: Option<&str>,
    ) -> Result<Client, String> {
        let mut ws = Self::connect_raw(url).await;
        let join = ClientMsg::Join {
            room: room.into(),
            password: password.map(String::from),
            name: name.into(),
        };
        ws.send(Message::text(serde_json::to_string(&join).unwrap()))
            .await
            .unwrap();
        let deadline = Instant::now() + T;
        let mut members = Vec::new();
        loop {
            let msg = recv(&mut ws, deadline).await.expect("no welcome");
            match msg {
                ServerMsg::Welcome {
                    you, room_state, ..
                } => {
                    return Ok(Client {
                        ws,
                        state: room_state,
                        members,
                        you,
                    })
                }
                ServerMsg::Members { list } => members = list,
                ServerMsg::Rejected { message } => return Err(message),
                _ => {}
            }
        }
    }

    async fn send(&mut self, m: ClientMsg) {
        self.ws
            .send(Message::text(serde_json::to_string(&m).unwrap()))
            .await
            .unwrap();
    }

    fn apply(&mut self, msg: &ServerMsg) {
        match msg {
            ServerMsg::State { room_state } => self.state = room_state.clone(),
            ServerMsg::Members { list } => self.members = list.clone(),
            _ => {}
        }
    }

    /// Waits for a message matching `pred` (applying all state updates on the way).
    async fn wait_for(&mut self, what: &str, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
        let deadline = Instant::now() + T;
        loop {
            let msg = recv(&mut self.ws, deadline)
                .await
                .unwrap_or_else(|| panic!("timeout waiting for {what}"));
            self.apply(&msg);
            if pred(&msg) {
                return msg;
            }
        }
    }

    /// Waits until a `State` satisfying `pred` arrives (or the current one does).
    async fn wait_state(&mut self, what: &str, pred: impl Fn(&RoomState) -> bool) -> RoomState {
        if pred(&self.state) {
            return self.state.clone();
        }
        let msg = self
            .wait_for(
                what,
                |m| matches!(m, ServerMsg::State { room_state } if pred(room_state)),
            )
            .await;
        match msg {
            ServerMsg::State { room_state } => room_state,
            _ => unreachable!(),
        }
    }

    /// Collects everything that arrives within `dur`.
    async fn drain(&mut self, dur: Duration) -> Vec<ServerMsg> {
        let deadline = Instant::now() + dur;
        let mut out = Vec::new();
        while let Some(msg) = recv(&mut self.ws, deadline).await {
            self.apply(&msg);
            out.push(msg);
        }
        out
    }

    /// Adds tracks through the Add -> ResolveRequest -> AddResolved flow.
    async fn add(&mut self, url: &str, tracks: Vec<TrackInfo>) {
        self.send(ClientMsg::Add { url: url.into() }).await;
        let req = self
            .wait_for("ResolveRequest", |m| {
                matches!(m, ServerMsg::ResolveRequest { .. })
            })
            .await;
        let ServerMsg::ResolveRequest { request_id, .. } = req else {
            unreachable!()
        };
        self.send(ClientMsg::AddResolved { request_id, tracks })
            .await;
    }
}

async fn recv(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    deadline: Instant,
) -> Option<ServerMsg> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                return Some(serde_json::from_str(t.as_str()).unwrap())
            }
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

fn track(id: &str, dur: Option<u64>) -> TrackInfo {
    TrackInfo {
        youtube_id: id.into(),
        title: format!("Pesma {id}"),
        duration_ms: dur,
    }
}

fn three_tracks() -> Vec<TrackInfo> {
    vec![
        track("aaaaaaaaaaa", Some(200_000)),
        track("bbbbbbbbbbb", Some(200_000)),
        track("ccccccccccc", Some(200_000)),
    ]
}

async fn three_clients(url: &str, room: &str) -> (Client, Client, Client) {
    let a = Client::join(url, room, "Ana", None).await.unwrap();
    let b = Client::join(url, room, "Bojan", None).await.unwrap();
    let c = Client::join(url, room, "Ceca", None).await.unwrap();
    (a, b, c)
}

/// All three clients report Ready for the current version and see Playing.
async fn all_ready(cs: &mut [&mut Client]) -> RoomState {
    for c in cs.iter_mut() {
        let st = c.wait_state("loading state", |s| s.waiting_for_ready).await;
        c.send(ClientMsg::Ready {
            version: st.playback.version,
        })
        .await;
    }
    let mut last = None;
    for c in cs.iter_mut() {
        last = Some(
            c.wait_state("playing", |s| s.playback.status == PlayStatus::Playing)
                .await,
        );
    }
    last.unwrap()
}

#[tokio::test]
async fn health_endpoint_returns_200() {
    let (_, addr) = start(fast_cfg()).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    assert!(buf.starts_with("HTTP/1.1 200"), "{buf}");
}

#[tokio::test]
async fn members_join_and_leave_broadcast() {
    let (url, _) = start(fast_cfg()).await;
    let (mut a, b, c) = three_clients(&url, "clanovi").await;
    a.wait_for(
        "3 members",
        |m| matches!(m, ServerMsg::Members { list } if list.len() == 3),
    )
    .await;
    assert_eq!(a.members.len(), 3);
    assert_ne!(a.you.id, b.you.id);
    drop(b);
    a.wait_for(
        "2 members",
        |m| matches!(m, ServerMsg::Members { list } if list.len() == 2),
    )
    .await;
    assert!(a.members.iter().any(|m| m.id == c.you.id));
}

#[tokio::test]
async fn add_play_pause_seek_broadcast_to_everyone() {
    let (url, _) = start(fast_cfg()).await;
    let (mut a, mut b, mut c) = three_clients(&url, "soba").await;

    a.add(
        "https://www.youtube.com/playlist?list=PLtest123",
        three_tracks(),
    )
    .await;
    // Adding to a stopped room starts the first new track in loading state.
    for cl in [&mut a, &mut b, &mut c] {
        let st = cl
            .wait_state("queue of 3", |s| s.queue.len() == 3 && s.waiting_for_ready)
            .await;
        assert_eq!(st.playback.track_index, Some(0));
        assert_eq!(st.playback.status, PlayStatus::Paused);
        assert_eq!(st.queue[0].added_by, "Ana");
    }

    let before = server::now_ms();
    let st = all_ready(&mut [&mut a, &mut b, &mut c]).await;
    // Scheduled start slightly in the future.
    assert!(
        st.playback.anchor_server_ms >= before + 90,
        "anchor should be in the future"
    );
    assert_eq!(st.playback.anchor_position_ms, 0);

    // Pause.
    tokio::time::sleep(Duration::from_millis(300)).await;
    b.send(ClientMsg::Pause).await;
    for cl in [&mut a, &mut b, &mut c] {
        let st = cl
            .wait_state("paused", |s| s.playback.status == PlayStatus::Paused)
            .await;
        assert!(st.playback.anchor_position_ms > 0 && st.playback.anchor_position_ms < 1000);
    }

    // Seek while paused stays paused.
    c.send(ClientMsg::Seek {
        position_ms: 42_000,
    })
    .await;
    for cl in [&mut a, &mut b, &mut c] {
        let st = cl
            .wait_state("seek 42s", |s| s.playback.anchor_position_ms == 42_000)
            .await;
        assert_eq!(st.playback.status, PlayStatus::Paused);
    }

    // Seek is clamped to duration.
    a.send(ClientMsg::Seek {
        position_ms: 999_999_999,
    })
    .await;
    a.wait_state("clamped", |s| s.playback.anchor_position_ms == 200_000)
        .await;
    a.send(ClientMsg::Seek {
        position_ms: 42_000,
    })
    .await;
    a.wait_state("seek back", |s| s.playback.anchor_position_ms == 42_000)
        .await;

    // Play resumes for everyone.
    a.send(ClientMsg::Play).await;
    for cl in [&mut a, &mut b, &mut c] {
        let st = cl
            .wait_state("resumed", |s| s.playback.status == PlayStatus::Playing)
            .await;
        assert_eq!(st.playback.anchor_position_ms, 42_000);
    }

    // Seek while playing goes through the ready barrier.
    b.send(ClientMsg::Seek {
        position_ms: 90_000,
    })
    .await;
    for cl in [&mut a, &mut b, &mut c] {
        cl.wait_state("seek loading", |s| {
            s.waiting_for_ready && s.playback.anchor_position_ms == 90_000
        })
        .await;
    }
    let st = all_ready(&mut [&mut a, &mut b, &mut c]).await;
    assert_eq!(st.playback.anchor_position_ms, 90_000);
}

#[tokio::test]
async fn simultaneous_double_skip_skips_once() {
    let (url, _) = start(fast_cfg()).await;
    let (mut a, mut b, mut c) = three_clients(&url, "skip").await;
    a.add("https://youtu.be/aaaaaaaaaaa", three_tracks()).await;
    all_ready(&mut [&mut a, &mut b, &mut c]).await;
    let v = a.state.playback.version;
    b.wait_state("sync b", |s| s.playback.version == v).await;

    // Both press "next" based on the same version at the same moment.
    let (ra, rb) = tokio::join!(
        a.send(ClientMsg::Next { version: Some(v) }),
        b.send(ClientMsg::Next { version: Some(v) })
    );
    let _ = (ra, rb);
    c.drain(Duration::from_millis(400)).await;
    assert_eq!(
        c.state.playback.track_index,
        Some(1),
        "must skip exactly once"
    );

    // A stale previous is ignored too.
    a.send(ClientMsg::Previous { version: Some(v) }).await;
    c.drain(Duration::from_millis(300)).await;
    assert_eq!(c.state.playback.track_index, Some(1));
}

#[tokio::test]
async fn ready_barrier_waits_for_all_then_times_out() {
    let (url, _) = start(fast_cfg()).await;
    let (mut a, mut b, mut c) = three_clients(&url, "barijera").await;
    a.add("https://youtu.be/aaaaaaaaaaa", three_tracks()).await;
    let st = c.wait_state("loading", |s| s.waiting_for_ready).await;
    let v = st.playback.version;
    let started = Instant::now();

    // Only two of three report ready: no start yet.
    a.send(ClientMsg::Ready { version: v }).await;
    b.send(ClientMsg::Ready { version: v }).await;
    let early = c.drain(Duration::from_millis(400)).await;
    assert!(
        !early.iter().any(|m| matches!(m, ServerMsg::State { room_state } if room_state.playback.status == PlayStatus::Playing)),
        "must not start before everyone is ready or the timeout"
    );

    // Timeout (800 ms in tests) starts playback anyway.
    let st = c
        .wait_state("timeout start", |s| {
            s.playback.status == PlayStatus::Playing
        })
        .await;
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(700),
        "started too early: {waited:?}"
    );
    assert!(!st.waiting_for_ready);

    // Next track: all ready -> immediate start (well before the timeout).
    a.wait_state("a playing", |s| s.playback.status == PlayStatus::Playing)
        .await;
    b.wait_state("b playing", |s| s.playback.status == PlayStatus::Playing)
        .await;
    a.send(ClientMsg::Next {
        version: Some(st.playback.version),
    })
    .await;
    let t0 = Instant::now();
    all_ready(&mut [&mut a, &mut b, &mut c]).await;
    assert!(t0.elapsed() < Duration::from_millis(600));
    assert_eq!(c.state.playback.track_index, Some(1));
}

#[tokio::test]
async fn ready_for_old_version_is_ignored() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(&url, "stari", "Ana", None).await.unwrap();
    a.add("https://youtu.be/aaaaaaaaaaa", three_tracks()).await;
    let st = a.wait_state("loading", |s| s.waiting_for_ready).await;
    a.send(ClientMsg::Ready {
        version: st.playback.version - 1,
    })
    .await;
    let msgs = a.drain(Duration::from_millis(300)).await;
    assert!(!msgs
        .iter()
        .any(|m| matches!(m, ServerMsg::State { room_state } if room_state.playback.status == PlayStatus::Playing)));
}

#[tokio::test]
async fn auto_advance_on_ended_and_on_server_timeline() {
    let (url, _) = start(fast_cfg()).await;
    let (mut a, mut b, mut c) = three_clients(&url, "kraj").await;
    a.add(
        "https://youtu.be/aaaaaaaaaaa",
        vec![
            track("aaaaaaaaaaa", Some(200_000)),
            track("bbbbbbbbbbb", Some(600)),
            track("ccccccccccc", Some(500)),
        ],
    )
    .await;
    let st = all_ready(&mut [&mut a, &mut b, &mut c]).await;
    let v = st.playback.version;

    // A stale Ended is ignored.
    b.send(ClientMsg::Ended { version: v - 1 }).await;
    c.drain(Duration::from_millis(200)).await;
    assert_eq!(c.state.playback.track_index, Some(0));

    // First Ended for the current version advances; the second (same version) does not.
    b.send(ClientMsg::Ended { version: v }).await;
    a.send(ClientMsg::Ended { version: v }).await;
    let st = c
        .wait_state("track 1", |s| s.playback.track_index == Some(1))
        .await;
    assert!(st.waiting_for_ready);
    c.drain(Duration::from_millis(150)).await;
    assert_eq!(c.state.playback.track_index, Some(1));

    // Nobody sends Ready or Ended: barrier timeout, then the server timeline
    // (600 ms + 200 ms grace) advances to track 2, then to Stopped.
    let st = c
        .wait_state("track 2", |s| s.playback.track_index == Some(2))
        .await;
    assert_eq!(st.queue[2].youtube_id, "ccccccccccc");
    let st = c
        .wait_state("stopped", |s| s.playback.status == PlayStatus::Stopped)
        .await;
    assert_eq!(st.playback.track_index, None);
    a.drain(Duration::from_millis(10)).await;

    // Adding a song to a stopped room starts it.
    a.add(
        "https://youtu.be/ddddddddddd",
        vec![track("ddddddddddd", Some(1000))],
    )
    .await;
    let st = c
        .wait_state("new track", |s| s.playback.track_index == Some(3))
        .await;
    assert!(st.waiting_for_ready);
}

#[tokio::test]
async fn remove_and_move_keep_current_track() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(&url, "red", "Ana", None).await.unwrap();
    a.add(
        "https://youtu.be/aaaaaaaaaaa",
        vec![
            track("aaaaaaaaaaa", Some(100_000)),
            track("bbbbbbbbbbb", Some(100_000)),
            track("ccccccccccc", Some(100_000)),
            track("ddddddddddd", Some(100_000)),
        ],
    )
    .await;
    let st = a.wait_state("4 tracks", |s| s.queue.len() == 4).await;
    let ids: Vec<_> = st.queue.iter().map(|t| t.id).collect();
    a.send(ClientMsg::PlayTrack { track_id: ids[2] }).await;
    a.wait_state("on c", |s| s.playback.track_index == Some(2))
        .await;
    let v = a.state.playback.version;

    // Remove a track before the current one: index shifts, version unchanged.
    a.send(ClientMsg::Remove { track_id: ids[0] }).await;
    let st = a.wait_state("3 tracks", |s| s.queue.len() == 3).await;
    assert_eq!(st.playback.track_index, Some(1));
    assert_eq!(st.current_track().unwrap().id, ids[2]);
    assert_eq!(st.playback.version, v);

    // Move the current track to the top.
    a.send(ClientMsg::Move {
        track_id: ids[2],
        new_index: 0,
    })
    .await;
    let st = a
        .wait_state("moved", |s| s.queue.first().map(|t| t.id) == Some(ids[2]))
        .await;
    assert_eq!(st.playback.track_index, Some(0));

    // Removing the current track plays the one now at its index.
    a.send(ClientMsg::Remove { track_id: ids[2] }).await;
    let st = a.wait_state("2 tracks", |s| s.queue.len() == 2).await;
    assert_eq!(st.current_track().unwrap().id, ids[1]);
    assert!(st.waiting_for_ready);
}

#[tokio::test]
async fn duration_fills_only_unknown_durations() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(&url, "trajanje", "Ana", None).await.unwrap();
    a.add(
        "https://youtu.be/aaaaaaaaaaa",
        vec![
            track("aaaaaaaaaaa", None),
            track("bbbbbbbbbbb", Some(5_000)),
            track("aaaaaaaaaaa", None),
        ],
    )
    .await;
    a.wait_state("3 tracks", |s| s.queue.len() == 3).await;
    let rev = a.state.revision;

    a.send(ClientMsg::Duration {
        youtube_id: "aaaaaaaaaaa".into(),
        duration_ms: 213_000,
    })
    .await;
    let st = a
        .wait_state("durations", |s| s.queue[0].duration_ms.is_some())
        .await;
    assert_eq!(st.queue[0].duration_ms, Some(213_000));
    assert_eq!(st.queue[2].duration_ms, Some(213_000));
    assert!(st.revision > rev);

    // Known durations are never overwritten; nonsense values are ignored.
    a.send(ClientMsg::Duration {
        youtube_id: "bbbbbbbbbbb".into(),
        duration_ms: 1,
    })
    .await;
    a.send(ClientMsg::Duration {
        youtube_id: "aaaaaaaaaaa".into(),
        duration_ms: 0,
    })
    .await;
    a.drain(Duration::from_millis(300)).await;
    assert_eq!(a.state.queue[1].duration_ms, Some(5_000));
    assert_eq!(a.state.queue[0].duration_ms, Some(213_000));
}

#[tokio::test]
async fn password_set_by_first_joiner() {
    let (url, _) = start(fast_cfg()).await;
    let _a = Client::join(&url, "Tajna", "Ana", Some("sifra"))
        .await
        .unwrap();
    let err = Client::join(&url, "tajna", "Bojan", Some("pogresna"))
        .await
        .err()
        .expect("wrong password must be rejected");
    assert!(err.contains("lozinka"));
    assert!(Client::join(&url, "tajna", "Bojan", None).await.is_err());
    let b = Client::join(&url, "TAJNA", "Bojan", Some("sifra"))
        .await
        .unwrap();
    assert_eq!(b.state.name, "tajna");
}

#[tokio::test]
async fn validation_limits() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(
        &url,
        "limiti",
        "Ime koje je mnogo duže od trideset dva znaka",
        None,
    )
    .await
    .unwrap();
    assert_eq!(a.you.name.chars().count(), 32);

    a.send(ClientMsg::Add {
        url: "https://vimeo.com/123".into(),
    })
    .await;
    a.wait_for("not youtube error", |m| {
        matches!(m, ServerMsg::Error { .. })
    })
    .await;

    // Resolved results with bad ids are dropped; unknown request ids are refused.
    a.send(ClientMsg::AddResolved {
        request_id: 999,
        tracks: vec![track("aaaaaaaaaaa", None)],
    })
    .await;
    a.wait_for("unknown request", |m| matches!(m, ServerMsg::Error { .. }))
        .await;
    a.add(
        "https://youtu.be/aaaaaaaaaaa",
        vec![track("bad id!", None), track("aaaaaaaaaaa", None)],
    )
    .await;
    let st = a.wait_state("1 track", |s| s.queue.len() == 1).await;
    assert_eq!(st.queue[0].youtube_id, "aaaaaaaaaaa");

    // Queue limit of 500.
    let many: Vec<TrackInfo> = (0..600).map(|_| track("bbbbbbbbbbb", Some(1000))).collect();
    a.add("https://www.youtube.com/playlist?list=PLbig", many)
        .await;
    let st = a.wait_state("500", |s| s.queue.len() == 500).await;
    assert_eq!(st.queue.len(), shared::limits::MAX_QUEUE);

    // Oversized message closes the connection.
    let mut raw = Client::connect_raw(&url).await;
    let big = format!(r#"{{"type":"Add","url":"{}"}}"#, "x".repeat(70 * 1024));
    let _ = raw.send(Message::text(big)).await;
    let closed = tokio::time::timeout(T, async {
        loop {
            match raw.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(closed);
}

#[tokio::test]
async fn rate_limit_kicks_in() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(&url, "brzo", "Ana", None).await.unwrap();
    for i in 0..100 {
        a.send(ClientMsg::Ping { client_ms: i }).await;
    }
    let msgs = a.drain(Duration::from_millis(500)).await;
    let pongs = msgs
        .iter()
        .filter(|m| matches!(m, ServerMsg::Pong { .. }))
        .count();
    assert!(pongs < 100, "some messages must be dropped");
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Error { .. })));
}

#[tokio::test]
async fn ping_pong_and_empty_room_cleanup() {
    let (url, _) = start(fast_cfg()).await;
    let mut a = Client::join(&url, "prazna", "Ana", Some("x"))
        .await
        .unwrap();
    a.send(ClientMsg::Ping { client_ms: 1234 }).await;
    let pong = a
        .wait_for("pong", |m| matches!(m, ServerMsg::Pong { .. }))
        .await;
    let ServerMsg::Pong {
        client_ms,
        server_ms,
    } = pong
    else {
        unreachable!()
    };
    assert_eq!(client_ms, 1234);
    assert!(server_ms > 0);
    a.add("https://youtu.be/aaaaaaaaaaa", three_tracks()).await;
    a.wait_state("added", |s| s.queue.len() == 3).await;
    a.send(ClientMsg::Leave).await;
    drop(a);

    // Room still exists (password kept) right after leaving...
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(Client::join(&url, "prazna", "Bojan", None).await.is_err());
    // ...and the queue was kept and paused.
    let b = Client::join(&url, "prazna", "Bojan", Some("x"))
        .await
        .unwrap();
    assert_eq!(b.state.queue.len(), 3);
    assert_ne!(b.state.playback.status, PlayStatus::Playing);
    drop(b);

    // After the TTL (2 s in tests) the room is gone: a new password can be set.
    tokio::time::sleep(Duration::from_millis(2600)).await;
    let c = Client::join(&url, "prazna", "Ceca", Some("nova"))
        .await
        .unwrap();
    assert!(c.state.queue.is_empty());
}
