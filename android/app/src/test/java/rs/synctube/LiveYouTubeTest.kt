package rs.synctube

import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import okhttp3.Request
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Hits YouTube for real. Run with: gradlew testDebugUnitTest -Dsynctube.live=1 */
class LiveYouTubeTest {
    private val live = System.getProperty("synctube.live") == "1"
    private val http = OkHttpClient()

    private fun fetch(url: String, range: String): Int {
        val b = Request.Builder().url(url).header("Range", range)
        Resolver.streamHeaders(url).forEach { (k, v) -> b.header(k, v) }
        http.newCall(b.build()).execute().use { r ->
            val n = r.body!!.byteStream().use { s ->
                val buf = ByteArray(65536); var total = 0L
                while (total < 4_000_000) { val k = s.read(buf); if (k < 0) break; total += k }
                total
            }
            println("GET $range -> ${r.code} $n bytes")
            return r.code
        }
    }

    @Test fun resolveAndFetch() = runBlocking {
        assumeTrue(live)
        val r = Resolver(http)
        val meta = r.metadata(YoutubeRef.Video("dQw4w9WgXcQ"))
        println("meta: $meta")
        val s = r.stream("dQw4w9WgXcQ")
        println("stream: ${s.url.take(120)}... dur=${s.durationMs}")
        assertTrue(fetch(s.url, "bytes=0-65535") in 200..206)
        assertTrue(fetch(s.url, "bytes=2000000-2065535") in 200..206)
        assertTrue(fetch(s.url, "bytes=0-") in 200..206)
        val hits = r.search("dubioza kolektiv", false)
        println("search: ${hits.take(3)}")
        assertTrue(hits.isNotEmpty())
        val pl = r.search("balkan hits", true)
        println("playlists: ${pl.take(2)}")
        val first = pl.first { it.playlistId != null }
        val tracks = r.metadata(YoutubeRef.Playlist(first.playlistId!!))
        println("playlist tracks: ${tracks.size}, first=${tracks.firstOrNull()}")
        assertTrue(tracks.isNotEmpty())
    }
}
