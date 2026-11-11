# SyncTube

Listen to YouTube music in sync with a few friends. One person pastes a link (a song or a whole playlist), and everyone hears the same thing at the same moment, each through their own speakers and at their own volume.

- `server/`: a small Rust WebSocket server (axum + tokio), meant to run on Render's free tier.
- `client/`: a desktop app for Windows and Linux (egui). It plays audio with **mpv** and resolves YouTube links with **yt-dlp**, and downloads both automatically on first run.
- `shared/`: the protocol and the sync math, shared by both.

## Download

| System | File |
|---|---|
| Windows | [synctube.exe](https://github.com/hcr2accc-cpu/synctube/releases/download/windows/synctube.exe): download and run it. |
| Linux (x86_64) | [synctube-linux-x86_64.tar.gz](https://github.com/hcr2accc-cpu/synctube/releases/download/linux/synctube-linux-x86_64.tar.gz): needs `mpv` installed, see [LINUX.md](LINUX.md). |

The downloads do not include a server address. Get it from whoever runs the server, paste it once under **Napredno**, and the app remembers it, along with the room and nickname.

## 1. Run the server locally

```powershell
cargo run --release -p server
```

The server listens on `0.0.0.0:8080`. Set a different port with the `PORT` env var and more logging with `RUST_LOG=debug`. It has two endpoints:

- `GET /health` returns `200 ok`.
- `ws://localhost:8080/ws` is the WebSocket endpoint.

In the client, enter `ws://127.0.0.1:8080/ws` as the server address. Friends on your LAN use `ws://<your-LAN-IP>:8080/ws`, which needs the port allowed in Windows Firewall.

## 2. Deploy the server to Render (free)

1. Create a GitHub repository and push the contents of this folder to it. `Dockerfile` and `render.yaml` must be at the repository root.
   ```powershell
   git init; git add .; git commit -m "SyncTube"
   git branch -M main
   git remote add origin https://github.com/<you>/synctube.git
   git push -u origin main
   ```
2. Sign in at <https://dashboard.render.com> with GitHub.
3. Create the service in one of two ways:
   - **New → Blueprint** (recommended): pick the repository. Render reads `render.yaml` and proposes a free web service called `synctube-server`. Click **Apply**.
   - **New → Web Service**: pick the repository, set **Language** to **Docker** and **Instance type** to **Free**, and set **Health Check Path** to `/health`. Leave everything else at the defaults; Render sets `PORT` itself.
4. Wait for the first build. Compiling Rust takes about 5–15 minutes. When it finishes, the status turns **Live** and the log shows `SyncTube server listening on 0.0.0.0:10000`.
5. Copy the service URL from the top of the page, for example `https://synctube-server-abcd.onrender.com`. The client needs the WebSocket form of that URL, with `wss://` in front and `/ws` at the end:
   ```
   wss://synctube-server-abcd.onrender.com/ws
   ```
   Opening `https://…onrender.com/health` in a browser should show `ok`.

Every push to `main` redeploys automatically.

## 3. Build the client

You need Rust (<https://rustup.rs>) and a Windows linker, from either of these toolchains:

- **MSVC** (default): install "Visual Studio Build Tools" with the *Desktop development with C++* workload.
- **GNU** (no Visual Studio): `rustup toolchain install stable-x86_64-pc-windows-gnu`, then run `rustup override set stable-x86_64-pc-windows-gnu` in this folder. A MinGW-w64 linker (for example from MSYS2) must be on `PATH`.

Then build:

```powershell
cargo build --release -p client
```

The result is `target\release\synctube.exe`.

To bake in your Render URL so friends don't have to type it, use the release script instead:

```powershell
.\build-release.ps1 -ServerUrl wss://synctube-server-abcd.onrender.com/ws
```

The script writes `dist\synctube.exe`.

### What to send friends

Send **only `synctube.exe`**. On first launch it does the following:

- Downloads `yt-dlp.exe` (from the official yt-dlp GitHub releases) and `mpv.exe` (the Windows build linked from mpv.io) into `%APPDATA%\SyncTube\bin\`, with a progress bar.
- Runs `yt-dlp -U` on every start, so YouTube fixes arrive automatically.
- Remembers the server address, room, nickname and volume in `%APPDATA%\SyncTube\config.json`.

Logs go to `%APPDATA%\SyncTube\synctube.log`.

If `yt-dlp.exe` or `mpv.exe` is already next to `synctube.exe`, the app uses those copies instead.

Windows SmartScreen may warn about an unsigned exe. Choose *More info → Run anyway*.

### Using it

Everyone enters the same **room** name, an optional password, and a nickname (the server address is under **Napredno**), then clicks **Poveži se**. Any member can do the following:

- Paste a YouTube song or playlist link and press Enter or **Dodaj**, or press **Ctrl+V anywhere** in the window to add the copied link at once.
- Play, pause or skip (**Space** toggles playback), and seek by clicking or dragging the progress bar.
- Double-click a song in the queue to play it. Hover a song to move it, remove it, or put it right after the current one, and filter the queue with the search box.

Volume is local to each person.

A pasted link shows up in the queue immediately. The title comes from YouTube's oEmbed endpoint in about half a second, so the song lands in the shared queue quickly. The adding client then fetches the duration with yt-dlp in the background and reports it to the server with a `Duration` message.

The UI uses the Inter font (SIL Open Font License, see `client/assets/fonts/OFL.txt`). For titles in other scripts (Cyrillic, Korean, Chinese or Japanese), it falls back to the Windows system fonts. Thumbnails are loaded from `i.ytimg.com`.

## Development

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

`synctube --fake-audio` runs the GUI with a simulated player, so no mpv or yt-dlp is needed.

`synctube-cli` is a headless client for testing sync from a terminal:

```powershell
cargo run -p client --bin synctube-cli -- --server ws://127.0.0.1:8080/ws --room test --name ana --fake-audio
```

Type `add <url>`, `play`, `pause`, `next`, `seek 30`, `status` or `quit`. The CLI prints drift, playback speed and ping every 2 s.

## How sync works

- **Clock sync**: the client sends 8 pings and keeps the one with the lowest round-trip time to estimate the server clock offset, then resyncs every 30 s.
- **Starting a track**: the server waits until every client reports the track is buffered (up to a few seconds), then schedules a start time a short moment in the future. Everyone starts at that server time.
- **Late joiners** seek straight to the current position.
- **Drift correction**: each second the client compares its player position with where it should be.
  - Under 80 ms: nothing happens.
  - 80 ms to 1 s: playback runs at 0.97× or 1.03× until the drift is back within 40 ms.
  - Over 1 s: the client seeks.
- **Failed tracks**: if a track can't be resolved or played, the client retries once, then shows a notice and skips the track.

## Known limitations

- **YouTube changes break playback from time to time.** When that happens, songs fail until yt-dlp publishes a fix. The client runs `yt-dlp -U` on every start, so restarting the app a day later usually fixes it.
- **The first connection is slow on Render's free tier.** The free service sleeps after about 15 minutes without traffic, so the first connection afterwards can take about a minute. The client shows "Budim server…" and keeps retrying for up to 90 s.
- Render's free tier restarts the server occasionally. Rooms live only in memory, so the queue is lost when that happens. Clients reconnect on their own.
- The client is Windows-only. Age-restricted or region-locked videos can't be played.
