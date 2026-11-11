//! YouTube URL validation and parsing (no regex dependency).

use crate::limits::MAX_URL_BYTES;

/// What a YouTube URL points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YoutubeRef {
    Video(String),
    Playlist(String),
}

const HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
    "www.youtu.be",
    "youtube-nocookie.com",
    "www.youtube-nocookie.com",
];

/// True if `id` looks like a YouTube video id (11 chars of `[A-Za-z0-9_-]`).
pub fn is_valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(is_id_byte)
}

fn is_valid_playlist_id(id: &str) -> bool {
    (2..=64).contains(&id.len()) && id.bytes().all(is_id_byte)
}

fn is_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Canonical watch URL for a video id.
pub fn watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// Canonical playlist URL.
pub fn playlist_url(list_id: &str) -> String {
    format!("https://www.youtube.com/playlist?list={list_id}")
}

/// Parses a YouTube video or playlist URL. Returns `None` for anything that
/// is not on a YouTube domain or has no usable id.
///
/// A `watch?v=..&list=..` URL is treated as the single video; only
/// `/playlist?list=..` (or a `list` without a `v`) is treated as a playlist.
pub fn parse_youtube_url(input: &str) -> Option<YoutubeRef> {
    let input = input.trim();
    if input.is_empty() || input.len() > MAX_URL_BYTES || input.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = if let Some(r) = input.strip_prefix("https://") {
        r
    } else if let Some(r) = input.strip_prefix("http://") {
        r
    } else {
        input
    };
    let (host_port, path_query) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = host_port.split(':').next()?.to_ascii_lowercase();
    if !HOSTS.contains(&host.as_str()) {
        return None;
    }
    let path_query = path_query.split('#').next().unwrap_or("");
    let (path, query) = match path_query.find('?') {
        Some(i) => (&path_query[..i], &path_query[i + 1..]),
        None => (path_query, ""),
    };
    let param = |name: &str| -> Option<&str> {
        query.split('&').find_map(|kv| {
            let mut it = kv.splitn(2, '=');
            (it.next()? == name).then(|| it.next().unwrap_or(""))
        })
    };

    if host.ends_with("youtu.be") {
        let id = path.trim_start_matches('/').split('/').next()?;
        return is_valid_video_id(id).then(|| YoutubeRef::Video(id.to_string()));
    }

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["watch"] => {
            if let Some(v) = param("v").filter(|v| is_valid_video_id(v)) {
                return Some(YoutubeRef::Video(v.to_string()));
            }
            param("list")
                .filter(|l| is_valid_playlist_id(l))
                .map(|l| YoutubeRef::Playlist(l.to_string()))
        }
        ["playlist"] => param("list")
            .filter(|l| is_valid_playlist_id(l))
            .map(|l| YoutubeRef::Playlist(l.to_string())),
        ["shorts", id] | ["embed", id] | ["live", id] | ["v", id] => {
            is_valid_video_id(id).then(|| YoutubeRef::Video(id.to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_video_urls() {
        let v = |s: &str| Some(YoutubeRef::Video(s.to_string()));
        assert_eq!(
            parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            v("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_url("youtube.com/watch?feature=x&v=dQw4w9WgXcQ"),
            v("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_url("https://youtu.be/dQw4w9WgXcQ?t=10"),
            v("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_url("https://m.youtube.com/shorts/dQw4w9WgXcQ"),
            v("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDx"),
            v("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_url("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"),
            v("dQw4w9WgXcQ")
        );
    }

    #[test]
    fn parses_playlists() {
        assert_eq!(
            parse_youtube_url(
                "https://www.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI"
            ),
            Some(YoutubeRef::Playlist(
                "PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI".into()
            ))
        );
        assert_eq!(
            parse_youtube_url("https://www.youtube.com/watch?list=PLabc123"),
            Some(YoutubeRef::Playlist("PLabc123".into()))
        );
    }

    #[test]
    fn rejects_non_youtube() {
        assert_eq!(
            parse_youtube_url("https://evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            parse_youtube_url("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            parse_youtube_url("https://www.youtube.com/watch?v=short"),
            None
        );
        assert_eq!(
            parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXc!"),
            None
        );
        assert_eq!(parse_youtube_url("file:///etc/passwd"), None);
        assert_eq!(parse_youtube_url(""), None);
        assert_eq!(
            parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ x"),
            None
        );
    }

    #[test]
    fn video_id_validation() {
        assert!(is_valid_video_id("dQw4w9WgXcQ"));
        assert!(is_valid_video_id("a-b_c-d_e-f"));
        assert!(!is_valid_video_id("dQw4w9WgXc"));
        assert!(!is_valid_video_id("--rm-rf/xyz"));
    }
}
