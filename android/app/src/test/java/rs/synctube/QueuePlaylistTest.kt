package rs.synctube

import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import okhttp3.OkHttpClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.io.DataInputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.ConcurrentLinkedQueue
import kotlin.concurrent.thread

/** A whole playlist goes to the queue in batches, against a scripted WebSocket peer. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class QueuePlaylistTest {
    @get:Rule val tmp = TemporaryFolder()

    private val socket = ServerSocket(0, 5, InetAddress.getByName("127.0.0.1"))
    /** Adding messages received, in order. */
    private val got = ConcurrentLinkedQueue<ClientMsg>()
    @Volatile private var answer: (ClientMsg) -> List<ServerMsg> = { emptyList() }
    private lateinit var engine: Engine
    private lateinit var playlist: String

    private fun id(n: Int) = "video%06d".format(n)

    @Before fun start() {
        thread(isDaemon = true) { runCatching { serve() } }
        val ctx = ApplicationProvider.getApplicationContext<android.app.Application>()
        engine = Engine(ctx, OkHttpClient(), Prefs(ctx), tmp.root)
        engine.save(
            SaveTo.New("Duga"),
            SaveItem.Songs((0 until 500).map { Song(id(it), "ž".repeat(180), 200_000) }),
        )
        playlist = engine.state.value.library.playlists.single().id
        engine.connect("ws://127.0.0.1:${socket.localPort}/ws", "soba", "", "Ana")
        until { engine.state.value.joined }
    }

    @After fun stop() {
        engine.leave()
        socket.close()
    }

    private fun until(ms: Long = 10_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + ms
        while (!done() && System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idle()
            Thread.sleep(5)
        }
        assertTrue("timed out; got ${got.size} messages", done())
    }

    private fun settle(ms: Long = 400) {
        val end = System.currentTimeMillis() + ms
        while (System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idle()
            Thread.sleep(5)
        }
    }

    @Test fun sendsAllBatchesInOrder() {
        var next = 100L
        answer = { m -> if (m is ClientMsg.Add) listOf(ServerMsg.ResolveRequest(next++, m.url)) else emptyList() }
        engine.queuePlaylist(playlist)
        assertEquals("shown as being added", listOf(true), engine.state.value.pendingAdds.map { it.playlist })
        until { got.filterIsInstance<ClientMsg.AddResolved>().sumOf { it.tracks.size } >= 500 }
        settle()
        val all = got.toList()
        assertTrue("${all.size}", all.size >= 6)
        // Every batch is asked for with its first song, only after the one before was sent.
        all.forEachIndexed { i, m ->
            if (i % 2 == 0) {
                assertEquals(ClientMsg.Add(YouTubeUrl.watchUrl((all[i + 1] as ClientMsg.AddResolved).tracks.first().youtubeId)), m)
            } else {
                assertEquals(100L + i / 2, (m as ClientMsg.AddResolved).requestId)
                assertTrue(Wire.encode(m).toByteArray().size < 41 * 1024)
            }
        }
        val sent = all.filterIsInstance<ClientMsg.AddResolved>().flatMap { it.tracks }.map { it.youtubeId }
        assertEquals((0 until 500).map(::id), sent)
        assertTrue(engine.state.value.pendingAdds.isEmpty())
    }

    @Test fun errorDropsTheRest() {
        var adds = 0
        answer = { m ->
            when {
                m !is ClientMsg.Add -> emptyList()
                ++adds == 1 -> listOf(ServerMsg.ResolveRequest(1, m.url))
                else -> listOf(ServerMsg.Error("Red je pun"))
            }
        }
        engine.queuePlaylist(playlist)
        until { engine.state.value.toasts.any { it.text == "Red je pun" } }
        settle()
        assertEquals(listOf("Add", "AddResolved", "Add"), got.map { it.javaClass.simpleName })
        assertTrue(engine.state.value.pendingAdds.isEmpty())
        // It can be tried again.
        answer = { emptyList() }
        engine.queuePlaylist(playlist)
        until { got.size == 4 }
        assertEquals(ClientMsg.Add(YouTubeUrl.watchUrl(id(0))), got.last())
    }

    // ----- the peer -----

    private fun serve() {
        val s = socket.accept()
        val input = DataInputStream(s.getInputStream())
        val out = s.getOutputStream()
        var key = ""
        while (true) {
            @Suppress("DEPRECATION") val line = input.readLine() ?: return
            if (line.isEmpty()) break
            if (line.startsWith("Sec-WebSocket-Key:", ignoreCase = true)) key = line.substringAfter(':').trim()
        }
        val sha = MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").toByteArray())
        val accept = Base64.getEncoder().encodeToString(sha)
        out.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: $accept\r\n\r\n".toByteArray())
        out.flush()
        while (true) {
            val op = input.readUnsignedByte() and 0x0f
            var len = (input.readUnsignedByte() and 0x7f).toLong()
            if (len == 126L) len = input.readUnsignedShort().toLong() else if (len == 127L) len = input.readLong()
            val mask = ByteArray(4).also(input::readFully)
            val data = ByteArray(len.toInt()).also(input::readFully)
            for (i in data.indices) data[i] = (data[i].toInt() xor mask[i % 4].toInt()).toByte()
            if (op == 8) return
            if (op != 1) continue
            when (val m = Wire.json.decodeFromString(ClientMsg.serializer(), String(data))) {
                is ClientMsg.Join -> send(out, ServerMsg.Welcome(Member("a", m.name), RoomState(m.room), 0))
                is ClientMsg.Add, is ClientMsg.AddResolved -> {
                    got += m
                    answer(m).forEach { send(out, it) }
                }
                else -> {}
            }
        }
    }

    private fun send(out: OutputStream, m: ServerMsg) {
        val data = Wire.json.encodeToString(ServerMsg.serializer(), m).toByteArray()
        check(data.size < 65536)
        val head = if (data.size < 126) byteArrayOf(0x81.toByte(), data.size.toByte())
        else byteArrayOf(0x81.toByte(), 126, (data.size shr 8).toByte(), data.size.toByte())
        out.write(head + data)
        out.flush()
    }
}
