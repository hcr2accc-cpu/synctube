//! All user-facing client strings (Serbian, Latin script). Change them here.

pub const APP_TITLE: &str = "SyncTube";

// Connect screen
pub const CONNECT_HEADING: &str = "Poveži se sa ekipom";
pub const SERVER_URL: &str = "Adresa servera";
pub const ROOM: &str = "Soba";
pub const PASSWORD: &str = "Lozinka (opciono)";
pub const NICKNAME: &str = "Nadimak";
pub const CONNECT: &str = "Poveži se";
pub const CANCEL: &str = "Otkaži";
pub const FILL_ALL: &str = "Popuni adresu servera, sobu i nadimak.";
pub const NORMALIZE: &str = "Ujednači jačinu";
pub const NORMALIZE_ON: &str =
    "Uključeno: sve pesme sviraju približno jednako glasno. Klikni da isključiš.";
pub const NORMALIZE_OFF: &str =
    "Isključeno: pesme sviraju originalnom jačinom. Klikni da uključiš.";
pub const RECENT_ROOMS: &str = "Nedavne sobe";
pub const RECENT_JOIN: &str = "Klikni da uđeš ponovo";
pub const RECENT_FORGET: &str = "Ukloni sa liste";
pub const BAD_SERVER_URL: &str = "Adresa servera mora početi sa ws:// ili wss://";

// Connection status
pub const CONNECTING: &str = "Povezivanje…";
pub const WAKING_SERVER: &str = "Budim server… (prvo povezivanje može trajati do minut)";
pub const CONNECTED: &str = "Povezano";
pub const RECONNECTING: &str = "Veza je prekinuta, ponovno povezivanje";
pub const SERVER_UNREACHABLE: &str = "Server nije dostupan. Proveri adresu i pokušaj ponovo.";
pub const OFFLINE: &str = "Nije povezano";

// Room screen
pub const LEAVE: &str = "Izađi";
pub const NOTHING_PLAYING: &str = "Ništa ne svira. Dodaj YouTube link ispod.";
pub const PASTE_LINK: &str = "Nalepi YouTube link ili ukucaj šta tražiš";
pub const SEARCH: &str = "Traži";
pub const SEARCH_TITLE: &str = "Pretraga";
pub const BACK_TO_QUEUE: &str = "Nazad na red";
pub const ADD_TO_QUEUE: &str = "Dodaj u red";
pub const IN_QUEUE: &str = "Već je u redu";
pub const SEARCHING: &str = "Tražim na YouTube-u…";
pub const SEARCH_NOTHING: &str = "Ništa nije pronađeno.";
pub const SEARCH_HINT: &str = "dupli klik ili + dodaje pesmu u red";
pub const ADD: &str = "Dodaj";
pub const ADDING: &str = "Učitavam link…";
pub const QUEUE: &str = "Red pesama";
pub const QUEUE_EMPTY: &str = "Red je prazan.";
pub const MEMBERS: &str = "U sobi";
pub const VOLUME: &str = "Jačina";
pub const ADDED_BY: &str = "dodao/la";
pub const PREVIOUS: &str = "Prethodna";
pub const NEXT: &str = "Sledeća";
pub const PLAY: &str = "Pusti";
pub const PAUSE: &str = "Pauza";
pub const REMOVE: &str = "Ukloni iz reda";
pub const MOVE_UP: &str = "Pomeri gore";
pub const MOVE_DOWN: &str = "Pomeri dole";
pub const PLAY_THIS: &str = "Pusti ovu pesmu";
pub const YOU: &str = "(ti)";

// Playback/sync status
pub const PHASE_RESOLVING: &str = "Pripremam pesmu…";
pub const PHASE_BUFFERING: &str = "Baferovanje…";
pub const PHASE_WAITING: &str = "Čekam ostale…";
pub const PHASE_STARTING: &str = "Krećemo…";
pub const PHASE_PAUSED: &str = "Pauzirano";
pub const PHASE_STOPPED: &str = "Zaustavljeno";
pub const SYNCED: &str = "Sinhronizovano";
pub const SYNCING: &str = "Usklađujem";
pub const CLOCK_SYNCING: &str = "Usklađujem sat…";

// Errors / toasts
pub const RESOLVE_FAILED: &str = "Ne mogu da učitam pesmu sa YouTube-a.";
pub const SEARCH_FAILED: &str = "Pretraga nije uspela.";
pub const RESOLVE_TIMEOUT: &str = "YouTube predugo ne odgovara.";
pub const NOT_YOUTUBE: &str = "To nije ispravan YouTube link.";
pub const NOTHING_FOUND: &str = "Na tom linku nema pesama.";
pub const YTDLP_MISSING: &str = "yt-dlp nije pronađen.";
pub const MPV_MISSING: &str = "mpv nije pronađen.";
pub const MPV_INSTALL_LINUX: &str = "mpv nije instaliran. Instaliraj ga (Ubuntu/Debian: sudo apt install mpv, Fedora: sudo dnf install mpv, Arch: sudo pacman -S mpv) pa klikni „Pokušaj ponovo“.";
pub const MPV_RESTARTED: &str = "Plejer se srušio, ponovo ga pokrećem.";
pub const SKIPPING_BROKEN: &str = "Pesma ne može da se pusti, prelazim na sledeću:";

// Dependencies / about
pub const DEPS_HEADING: &str = "Preuzimam potrebne alate";
pub const DEPS_EXPLAIN: &str =
    "Samo pri prvom pokretanju: yt-dlp i mpv se preuzimaju sa zvaničnih GitHub stranica.";
pub const DEPS_FAILED: &str = "Preuzimanje nije uspelo:";
pub const RETRY: &str = "Pokušaj ponovo";
pub const DEP_WAITING: &str = "čeka";
pub const DEP_DOWNLOADING: &str = "preuzimam";
pub const DEP_EXTRACTING: &str = "raspakujem";
pub const DEP_VERIFYING: &str = "proveravam";
pub const DEP_DONE: &str = "spremno";
pub const ABOUT: &str = "O programu";
pub const YTDLP_VERSION: &str = "yt-dlp verzija";
pub const YTDLP_UPDATING: &str = "proveravam ažuriranja…";
pub const APP_VERSION: &str = "Verzija";
pub const FAKE_AUDIO: &str = "Simulirani zvuk (--fake-audio)";

pub const NOW_PLAYING: &str = "Sada svira";
pub const SEEK_HINT: &str = "Klikni da premotaš";
pub const QUEUE_FULL_HINT: &str = "Klikni na pesmu da je pustiš.";
pub const DEPS_CHECKING: &str = "Pokrećem…";
pub const SERVER_URL_HINT: &str = "wss://…onrender.com/ws";
pub const MISSING_TOOLS_PLAYBACK: &str = "Bez yt-dlp i mpv nema zvuka.";

// Redesigned UI
pub const ADVANCED: &str = "Napredno";
pub const ROOM_HINT: &str = "npr. ekipa";
pub const NICK_HINT: &str = "kako da te zovu";
pub const NOTHING_PLAYING_TITLE: &str = "Ništa ne svira";
pub const NOTHING_PLAYING_HINT: &str = "Potraži pesmu ili nalepi YouTube link ispod.";
pub const PASTE_HINT: &str =
    "Enter dodaje link ili traži · Ctrl+V bilo gde u prozoru odmah dodaje link · Razmak pušta/pauzira";
pub const SEARCH_QUEUE: &str = "Pretraži red…";
pub const JUMP_TO_CURRENT: &str = "Idi na trenutnu pesmu";
pub const PLAY_NEXT: &str = "Pusti posle trenutne";
pub const DOUBLE_CLICK_HINT: &str = "prevuci za redosled · klik na sliku pušta";
pub const LOADING_PLAYLIST: &str = "Učitavam plejlistu…";
pub const LOADING_TRACK: &str = "Učitavam pesmu…";
pub const QUEUE_EMPTY_TITLE: &str = "Red je prazan";
pub const QUEUE_EMPTY_HINT: &str =
    "Potraži pesmu ili nalepi YouTube link ispod (Ctrl+V radi bilo gde u prozoru).";
pub const NO_MATCHES: &str = "Nijedna pesma ne odgovara pretrazi.";
pub const SYNC_TITLE: &str = "Sinhronizacija";
pub const STATUS: &str = "Status";
pub const PING: &str = "Ping";
pub const DRIFT: &str = "Odstupanje";
pub const CLOCK: &str = "Sat";
pub const CLOCK_OK: &str = "usklađen";
pub const MUTE: &str = "Isključi zvuk";
pub const UNMUTE: &str = "Uključi zvuk";
pub const IDLE: &str = "Spremno";

// Sync dots (after a person's name)
pub const DOT_SYNCED: &str = "u taktu";
pub const DOT_LOADING: &str = "učitava";
pub const DOT_WAITING: &str = "čeka";

/// "žuri 120 ms" (ahead) / "kasni 120 ms" (behind).
pub fn dot_drift(ms: i32) -> String {
    if ms > 0 {
        format!("žuri {ms} ms")
    } else {
        format!("kasni {} ms", -ms)
    }
}

// Search modes
pub const SEARCH_SONGS: &str = "Pesme";
pub const SEARCH_PLAYLISTS: &str = "Plejliste";
pub const PLAYLIST_TAG: &str = "plejlista";
pub const SEARCH_HINT_PLAYLISTS: &str = "dupli klik ili + dodaje celu plejlistu u red";
pub const ADD_PLAYLIST: &str = "Dodaj celu plejlistu u red";

// Settings
pub const SETTINGS: &str = "Podešavanja";
pub const CLOSE: &str = "Zatvori";
pub const EQ_TITLE: &str = "Ekvilajzer";
pub const EQ_HINT: &str = "Prevuci klizač · dupli klik vraća na 0 dB · menja se samo kod tebe";
pub const EQ_RESET: &str = "Resetuj";
pub const EQ_CUSTOM: &str = "Prilagođeno";
pub const VIDEO_TITLE: &str = "Video režim";
pub const VIDEO_EXPLAIN: &str =
    "Prikazuje spot u aplikaciji, uporedo sa muzikom (dupli klik na video = ceo ekran, Esc za izlaz).";
pub const VIDEO_FULL_HINT: &str = "Dupli klik = ceo ekran";
pub const EXIT_FULLSCREEN: &str = "Izađi iz celog ekrana (Esc)";
pub const ON: &str = "Uključeno";
pub const OFF: &str = "Isključeno";

/// "1 pesma", "3 pesme", "7 pesama".
pub fn songs(n: usize) -> String {
    let word = if n % 10 == 1 && n % 100 != 11 {
        "pesma"
    } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
        "pesme"
    } else {
        "pesama"
    };
    format!("{n} {word}")
}

pub fn members_title(n: usize) -> String {
    format!("{MEMBERS} · {n}")
}

pub fn reconnecting_in(secs: u64) -> String {
    format!("{RECONNECTING} za {secs} s…")
}

pub fn drift(ms: i64) -> String {
    format!("{ms:+} ms")
}

pub fn rtt(ms: u64) -> String {
    format!("ping {ms} ms")
}

pub fn queue_count(n: usize) -> String {
    format!("{QUEUE} ({n})")
}
