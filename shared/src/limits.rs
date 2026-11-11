//! Limits enforced by the server (and pre-checked by the client).

/// Maximum number of tracks in a room queue.
pub const MAX_QUEUE: usize = 500;
/// Maximum nickname length in characters.
pub const MAX_NAME_CHARS: usize = 32;
/// Maximum room name length in characters.
pub const MAX_ROOM_CHARS: usize = 32;
/// Maximum room password length in characters.
pub const MAX_PASSWORD_CHARS: usize = 64;
/// Maximum track title length in characters (longer titles are truncated).
pub const MAX_TITLE_CHARS: usize = 200;
/// Maximum URL length in bytes.
pub const MAX_URL_BYTES: usize = 2048;
/// Maximum WebSocket message size in bytes.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Sustained messages per second allowed per connection.
pub const RATE_LIMIT_PER_SEC: u32 = 20;
/// Burst size of the per-connection token bucket.
pub const RATE_LIMIT_BURST: u32 = 40;

/// Trims and truncates `s` to at most `max` characters.
pub fn clamp_chars(s: &str, max: usize) -> String {
    s.trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_handles_unicode() {
        assert_eq!(clamp_chars("  ćevapčići  ", 4), "ćeva");
        assert_eq!(clamp_chars("ab", 32), "ab");
        assert_eq!(clamp_chars("   ", 32), "");
    }
}
