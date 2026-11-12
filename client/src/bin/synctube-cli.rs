//! Headless client for testing sync without the GUI.
//!
//! ```text
//! synctube-cli --server ws://127.0.0.1:8080/ws --room test --name ana [--fake-audio] [--fake-rate 1.02]
//! ```
//! Commands on stdin: add <url> | play | pause | next | prev | seek <sec> |
//! rm <n> | mv <n> <to> | vol <0-100> | status | quit

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use client::engine::{self, Command, EngineConfig, View};
use client::paths::Tools;

struct Args {
    server: String,
    room: String,
    name: String,
    password: String,
    fake_audio: bool,
    fake_rate: f64,
    no_update: bool,
    volume: f64,
    normalize: bool,
    status_every: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        server: "ws://127.0.0.1:8080/ws".into(),
        room: "test".into(),
        name: format!("cli-{}", std::process::id()),
        password: String::new(),
        fake_audio: false,
        fake_rate: 1.0,
        no_update: false,
        volume: 70.0,
        normalize: false,
        status_every: Some(2.0),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--server" => a.server = val()?,
            "--room" => a.room = val()?,
            "--name" => a.name = val()?,
            "--password" => a.password = val()?,
            "--fake-audio" => a.fake_audio = true,
            "--fake-rate" => a.fake_rate = val()?.parse().map_err(|_| "bad --fake-rate")?,
            "--no-update" => a.no_update = true,
            "--normalize" => a.normalize = true,
            "--volume" => a.volume = val()?.parse().map_err(|_| "bad --volume")?,
            "--status-every" => {
                let s: f64 = val()?.parse().map_err(|_| "bad --status-every")?;
                a.status_every = (s > 0.0).then_some(s);
            }
            "-h" | "--help" => {
                println!(
                    "synctube-cli --server URL --room ROOM --name NAME [--password P] \
                     [--fake-audio] [--fake-rate R] [--no-update] [--normalize] [--volume 0-100] [--status-every SECS]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn status_line(v: &View) -> String {
    let room = v.room.as_ref();
    let track = room.and_then(|r| r.current_track());
    let pos = v.position_ms().map_or("-".into(), fmt_ms);
    let player = v.player_pos_ms.map_or("-".into(), fmt_ms);
    format!(
        "[{:?}] {:?} status={:?} track={} pos={} player={} drift={} speed={:.2} rtt={} offset={} queue={} members={}",
        v.conn,
        v.phase,
        room.map(|r| r.playback.status),
        track.map_or("-", |t| t.title.as_str()),
        pos,
        player,
        v.drift_ms.map_or("-".into(), |d| format!("{d:+}ms")),
        v.speed,
        v.rtt_ms.map_or("-".into(), |r| format!("{r}ms")),
        v.offset_ms,
        room.map_or(0, |r| r.queue.len()),
        v.members.len(),
    )
}

fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}.{:03}", s / 60, s % 60, ms % 1000)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    client::logging::init(true);
    let cfg = EngineConfig {
        fake_audio: args.fake_audio,
        fake_rate: args.fake_rate,
        tools: Tools::discover(),
        volume: args.volume,
        normalize: args.normalize,
        auto_update: !args.no_update && !args.fake_audio,
        eq: client::eq::EqSettings::default(),
        video: false,
        video_wid: None,
    };
    let handle = engine::start(cfg, Arc::new(|| {}));
    handle.send(Command::Connect {
        url: args.server.clone(),
        room: args.room.clone(),
        password: args.password.clone(),
        name: args.name.clone(),
    });

    let view = handle.view.clone();
    if let Some(every) = args.status_every {
        std::thread::spawn(move || {
            let mut toasts_seen = 0;
            loop {
                std::thread::sleep(Duration::from_secs_f64(every));
                let v = view.lock().unwrap().clone();
                for t in v.toasts.iter().skip(toasts_seen) {
                    println!("toast{}: {}", if t.error { " (error)" } else { "" }, t.text);
                }
                toasts_seen = v.toasts.len();
                println!("{}", status_line(&v));
            }
        });
    }

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        let v = handle.view();
        let track_at = |i: &str| -> Option<uuid::Uuid> {
            let i: usize = i.parse().ok()?;
            v.room.as_ref()?.queue.get(i).map(|t| t.id)
        };
        let c = match (cmd, rest.as_slice()) {
            ("add", [url]) => Command::Add((*url).to_string()),
            ("play", []) => Command::Play,
            ("pause", []) => Command::Pause,
            ("next", []) => Command::Next,
            ("prev", []) => Command::Previous,
            ("seek", [s]) => match s.parse::<f64>() {
                Ok(s) => Command::Seek((s * 1000.0) as u64),
                Err(_) => continue,
            },
            ("goto", [i]) => match track_at(i) {
                Some(id) => Command::PlayTrack(id),
                None => continue,
            },
            ("rm", [i]) => match track_at(i) {
                Some(id) => Command::Remove(id),
                None => continue,
            },
            ("mv", [i, to]) => match (track_at(i), to.parse()) {
                (Some(id), Ok(to)) => Command::Move(id, to),
                _ => continue,
            },
            ("vol", [x]) => match x.parse() {
                Ok(x) => Command::SetVolume(x),
                Err(_) => continue,
            },
            ("status", []) => {
                println!("{}", status_line(&v));
                if let Some(r) = &v.room {
                    for (i, t) in r.queue.iter().enumerate() {
                        let mark = if r.playback.track_index == Some(i) {
                            ">"
                        } else {
                            " "
                        };
                        println!("{mark}{i:3} {} ({})", t.title, t.added_by);
                    }
                }
                continue;
            }
            ("quit", []) | ("exit", []) => break,
            _ => {
                eprintln!("unknown command");
                continue;
            }
        };
        handle.send(c);
    }
    handle.send(Command::Leave);
    std::thread::sleep(Duration::from_millis(300));
    handle.shutdown();
}
