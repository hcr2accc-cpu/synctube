package rs.synctube

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class YouTubeUrlTest {
    private fun v(id: String) = YoutubeRef.Video(id)

    @Test fun videos() {
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ"))
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("youtube.com/watch?feature=x&v=dQw4w9WgXcQ"))
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("https://youtu.be/dQw4w9WgXcQ?t=10"))
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("https://m.youtube.com/shorts/dQw4w9WgXcQ"))
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDx"))
        assertEquals(v("dQw4w9WgXcQ"), YouTubeUrl.parse("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"))
    }

    @Test fun playlists() {
        assertEquals(
            YoutubeRef.Playlist("PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI"),
            YouTubeUrl.parse("https://www.youtube.com/playlist?list=PLFgquLnL59alCl_2TQvOiD5Vgm1hCaGSI"),
        )
        assertEquals(YoutubeRef.Playlist("PLabc123"), YouTubeUrl.parse("https://www.youtube.com/watch?list=PLabc123"))
    }

    @Test fun rejects() {
        assertNull(YouTubeUrl.parse("https://evil.com/watch?v=dQw4w9WgXcQ"))
        assertNull(YouTubeUrl.parse("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"))
        assertNull(YouTubeUrl.parse("https://www.youtube.com/watch?v=short"))
        assertNull(YouTubeUrl.parse("https://www.youtube.com/watch?v=dQw4w9WgXc!"))
        assertNull(YouTubeUrl.parse("file:///etc/passwd"))
        assertNull(YouTubeUrl.parse(""))
        assertNull(YouTubeUrl.parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ x"))
        assertTrue(YouTubeUrl.isValidVideoId("a-b_c-d_e-f"))
        assertFalse(YouTubeUrl.isValidVideoId("--rm-rf/xyz"))
    }
}
