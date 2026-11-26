package rs.synctube

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ProtocolTest {
    @Test fun clientShapes() {
        assertEquals("""{"type":"Seek","position_ms":1500}""", Wire.encode(ClientMsg.Seek(1500)))
        assertEquals("""{"type":"Play"}""", Wire.encode(ClientMsg.Play))
        assertEquals("""{"type":"Next"}""", Wire.encode(ClientMsg.Next(null)))
        assertEquals("""{"type":"Next","version":3}""", Wire.encode(ClientMsg.Next(3)))
        assertEquals("""{"type":"Join","room":"ekipa","name":"Ana","radio":true}""", Wire.encode(ClientMsg.Join("ekipa", null, "Ana")))
        assertEquals("""{"type":"Status","loading":true}""", Wire.encode(ClientMsg.Status(true, null)))
        assertEquals(
            """{"type":"AddResolved","request_id":7,"tracks":[{"youtube_id":"dQw4w9WgXcQ","title":"T","duration_ms":1000}]}""",
            Wire.encode(ClientMsg.AddResolved(7, listOf(TrackInfo("dQw4w9WgXcQ", "T", 1000)))),
        )
    }

    @Test fun serverParses() {
        val s = """{"type":"State","room_state":{"name":"ekipa","queue":[{"id":"6f1c","youtube_id":"dQw4w9WgXcQ","title":"Pesma","duration_ms":null,"added_by":"Ana"}],"playback":{"track_index":0,"status":"Playing","anchor_server_ms":10,"anchor_position_ms":20,"version":3},"revision":7,"waiting_for_ready":false}}"""
        val m = Wire.decode(s) as ServerMsg.State
        assertEquals(PlayStatus.Playing, m.roomState.playback.status)
        assertEquals("Pesma", m.roomState.currentTrack()!!.title)
        assertNull(m.roomState.queue[0].durationMs)
        val w = Wire.decode("""{"type":"Welcome","you":{"id":"x","name":"Ana","sync":{"loading":false,"drift_ms":null}},"room_state":{"name":"a","queue":[],"playback":{"track_index":null,"status":"Stopped","anchor_server_ms":0,"anchor_position_ms":0,"version":0},"revision":0},"server_ms":5}""")
        assertTrue(w is ServerMsg.Welcome)
        assertNull(Wire.decode("""{"type":"SomethingNew","x":1}"""))
        assertTrue(Wire.decode("""{"type":"Members","list":[{"id":"a","name":"B"}]}""") is ServerMsg.Members)
    }
}
