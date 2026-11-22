// YouTube URL validation and parsing. Mirrors shared/src/youtube.rs.
package rs.synctube

sealed class YoutubeRef {
    data class Video(val id: String) : YoutubeRef()
    data class Playlist(val id: String) : YoutubeRef()
}

object YouTubeUrl {
    private const val MAX_URL_BYTES = 2048
    private val HOSTS = setOf(
        "youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com",
        "youtu.be", "www.youtu.be", "youtube-nocookie.com", "www.youtube-nocookie.com",
    )

    private fun isIdChar(c: Char) = c in 'a'..'z' || c in 'A'..'Z' || c in '0'..'9' || c == '_' || c == '-'
    fun isValidVideoId(id: String) = id.length == 11 && id.all(::isIdChar)
    private fun isValidPlaylistId(id: String) = id.length in 2..64 && id.all(::isIdChar)

    fun watchUrl(id: String) = "https://www.youtube.com/watch?v=$id"
    fun playlistUrl(id: String) = "https://www.youtube.com/playlist?list=$id"
    fun thumbUrl(id: String) = "https://i.ytimg.com/vi/$id/mqdefault.jpg"

    fun canonical(r: YoutubeRef) = when (r) {
        is YoutubeRef.Video -> watchUrl(r.id)
        is YoutubeRef.Playlist -> playlistUrl(r.id)
    }

    fun parse(raw: String): YoutubeRef? {
        val input = raw.trim()
        if (input.isEmpty() || input.toByteArray().size > MAX_URL_BYTES || input.any { it.isWhitespace() }) return null
        val rest = when {
            input.startsWith("https://") -> input.substring(8)
            input.startsWith("http://") -> input.substring(7)
            else -> input
        }
        val cut = rest.indexOfFirst { it == '/' || it == '?' || it == '#' }
        val hostPort = if (cut >= 0) rest.substring(0, cut) else rest
        var pathQuery = if (cut >= 0) rest.substring(cut) else ""
        val host = hostPort.substringBefore(':').lowercase()
        if (host !in HOSTS) return null
        pathQuery = pathQuery.substringBefore('#')
        val path = pathQuery.substringBefore('?')
        val query = if (pathQuery.contains('?')) pathQuery.substringAfter('?') else ""
        fun param(name: String): String? = query.split('&').firstNotNullOfOrNull { kv ->
            val parts = kv.split('=', limit = 2)
            if (parts[0] == name) parts.getOrElse(1) { "" } else null
        }
        if (host.endsWith("youtu.be")) {
            val id = path.trimStart('/').substringBefore('/')
            return if (isValidVideoId(id)) YoutubeRef.Video(id) else null
        }
        val seg = path.split('/').filter { it.isNotEmpty() }
        return when {
            seg == listOf("watch") -> {
                param("v")?.takeIf(::isValidVideoId)?.let { YoutubeRef.Video(it) }
                    ?: param("list")?.takeIf(::isValidPlaylistId)?.let { YoutubeRef.Playlist(it) }
            }
            seg == listOf("playlist") -> param("list")?.takeIf(::isValidPlaylistId)?.let { YoutubeRef.Playlist(it) }
            seg.size == 2 && seg[0] in setOf("shorts", "embed", "live", "v") ->
                seg[1].takeIf(::isValidVideoId)?.let { YoutubeRef.Video(it) }
            else -> null
        }
    }
}
