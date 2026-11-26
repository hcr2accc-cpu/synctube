package rs.synctube

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class UpdaterTest {
    @Test
    fun versions() {
        assertEquals(listOf(0L, 7L, 0L), Updater.parseVersion("v0.7"))
        assertEquals(listOf(0L, 6L, 0L), Updater.parseVersion("0.6.0"))
        assertNull(Updater.parseVersion("windows"))
        assertNull(Updater.parseVersion("v1"))
        assertNull(Updater.parseVersion("v1.2-beta"))
    }

    private fun release(tag: String, vararg assets: String, pre: Boolean = false) =
        """{"tag_name":"$tag","draft":false,"prerelease":$pre,"assets":[""" +
            assets.joinToString(",") { """{"name":"$it","browser_download_url":"https://x/$tag/$it","size":10}""" } + "]}"

    @Test
    fun picksNewestApk() {
        val json = Json.parseToJsonElement(
            "[" + listOf(
                release("android", "synctube.apk"),
                release("v0.6", "synctube.apk"),
                release("v0.10", "synctube.exe"),
                release("v0.9", "synctube.apk"),
                release("v1.0", "synctube.apk", pre = true),
            ).joinToString(",") + "]",
        )
        val u = Updater.pick(json, "0.6.0", "synctube.apk")!!
        assertEquals("v0.9", u.tag)
        assertEquals("https://x/v0.9/synctube.apk", u.url)
        assertEquals(10L, u.size)
        assertNull(Updater.pick(json, "0.9.0", "synctube.apk"))
    }

    @Test
    fun radioPickSkipsQueuedAndRepeats() {
        val songs = listOf("a", "b", "b", "c", "d", "e", "f", "g").map { Song(it, it, 1000) }
        assertEquals(listOf("b", "c", "d", "e", "f"), radioPick(songs, setOf("a")).map { it.youtubeId })
    }

    @Test
    fun radioMessages() {
        val m = Wire.decode("""{"type":"RadioRequest","request_id":4,"seeds":["dQw4w9WgXcQ"]}""")
        assertEquals(ServerMsg.RadioRequest(4, listOf("dQw4w9WgXcQ")), m)
        val st = Wire.decode("""{"type":"State","room_state":{"name":"a","radio":true}}""") as ServerMsg.State
        assertEquals(true, st.roomState.radio)
        val old = Wire.decode("""{"type":"State","room_state":{"name":"a"}}""") as ServerMsg.State
        assertNull(old.roomState.radio)
        val join = Wire.encode(ClientMsg.Join("a", null, "b"))
        assertEquals(true, join.contains("\"radio\":true"))
        assertEquals("""{"type":"SetRadio","on":false}""", Wire.encode(ClientMsg.SetRadio(false)))
    }
}
