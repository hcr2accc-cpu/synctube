//! User-facing strings sent by the server (Serbian, Latin script).

pub const WRONG_PASSWORD: &str = "Pogrešna lozinka za ovu sobu.";
pub const BAD_ROOM: &str = "Naziv sobe nije ispravan (1–32 znaka).";
pub const BAD_NAME: &str = "Nadimak nije ispravan (1–32 znaka).";
pub const ROOM_FULL: &str = "Soba je puna.";
pub const JOIN_FIRST: &str = "Prvo se pridruži sobi.";
pub const ALREADY_JOINED: &str = "Već si u sobi.";
pub const BAD_MESSAGE: &str = "Neispravna poruka.";
pub const RATE_LIMITED: &str = "Previše komandi, uspori malo.";
pub const NOT_YOUTUBE: &str = "To nije ispravan YouTube link.";
pub const QUEUE_FULL: &str = "Red je pun (najviše 500 pesama).";
pub const TOO_MANY_PENDING: &str = "Sačekaj da se prethodni linkovi učitaju.";
pub const UNKNOWN_REQUEST: &str = "Zahtev za dodavanje je istekao, pokušaj ponovo.";
pub const NOTHING_ADDED: &str = "Nijedna pesma nije dodata.";
pub const SERVER_BUSY: &str = "Server je zauzet, pokušaj ponovo.";

pub fn joined(name: &str) -> String {
    format!("{name} je ušao/la u sobu")
}

pub fn left(name: &str) -> String {
    format!("{name} je napustio/la sobu")
}

pub fn added_one(name: &str, title: &str) -> String {
    format!("{name} je dodao/la: {title}")
}

pub fn added_many(name: &str, count: usize) -> String {
    format!("{name} je dodao/la {count} pesama")
}

pub fn truncated(count: usize) -> String {
    format!("Red je pun, dodato je samo {count} pesama.")
}
