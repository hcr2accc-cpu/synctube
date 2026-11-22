// Wire protocol: JSON over WebSocket, tagged with "type". Mirrors shared/src/protocol.rs.
package rs.synctube

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

@Serializable
data class Track(
    val id: String,
    @SerialName("youtube_id") val youtubeId: String,
    val title: String,
    @SerialName("duration_ms") val durationMs: Long? = null,
    @SerialName("added_by") val addedBy: String = "",
)

@Serializable
data class TrackInfo(
    @SerialName("youtube_id") val youtubeId: String,
    val title: String,
    @SerialName("duration_ms") val durationMs: Long? = null,
)

@Serializable
enum class PlayStatus { Playing, Paused, Stopped }

@Serializable
data class PlaybackState(
    @SerialName("track_index") val trackIndex: Int? = null,
    val status: PlayStatus = PlayStatus.Stopped,
    @SerialName("anchor_server_ms") val anchorServerMs: Long = 0,
    @SerialName("anchor_position_ms") val anchorPositionMs: Long = 0,
    val version: Long = 0,
) {
    fun positionAt(serverNowMs: Long): Long = Sync.expectedPositionMs(this, serverNowMs)
}

@Serializable
data class MemberSync(
    val loading: Boolean = false,
    @SerialName("drift_ms") val driftMs: Int? = null,
)

@Serializable
data class Member(
    val id: String,
    val name: String,
    val sync: MemberSync? = null,
)

@Serializable
data class RoomState(
    val name: String = "",
    val queue: List<Track> = emptyList(),
    val playback: PlaybackState = PlaybackState(),
    val revision: Long = 0,
    @SerialName("waiting_for_ready") val waitingForReady: Boolean = false,
) {
    fun currentTrack(): Track? = playback.trackIndex?.let { queue.getOrNull(it) }
}

@Serializable
sealed class ClientMsg {
    @Serializable @SerialName("Join")
    data class Join(val room: String, val password: String? = null, val name: String) : ClientMsg()

    @Serializable @SerialName("Add")
    data class Add(val url: String) : ClientMsg()

    @Serializable @SerialName("AddResolved")
    data class AddResolved(@SerialName("request_id") val requestId: Long, val tracks: List<TrackInfo>) : ClientMsg()

    @Serializable @SerialName("Duration")
    data class Duration(@SerialName("youtube_id") val youtubeId: String, @SerialName("duration_ms") val durationMs: Long) : ClientMsg()

    @Serializable @SerialName("Play")
    data object Play : ClientMsg()

    @Serializable @SerialName("Pause")
    data object Pause : ClientMsg()

    @Serializable @SerialName("Seek")
    data class Seek(@SerialName("position_ms") val positionMs: Long) : ClientMsg()

    @Serializable @SerialName("Next")
    data class Next(val version: Long? = null) : ClientMsg()

    @Serializable @SerialName("Previous")
    data class Previous(val version: Long? = null) : ClientMsg()

    @Serializable @SerialName("PlayTrack")
    data class PlayTrack(@SerialName("track_id") val trackId: String) : ClientMsg()

    @Serializable @SerialName("Remove")
    data class Remove(@SerialName("track_id") val trackId: String) : ClientMsg()

    @Serializable @SerialName("Move")
    data class Move(@SerialName("track_id") val trackId: String, @SerialName("new_index") val newIndex: Int) : ClientMsg()

    @Serializable @SerialName("Ready")
    data class Ready(val version: Long) : ClientMsg()

    @Serializable @SerialName("Ended")
    data class Ended(val version: Long) : ClientMsg()

    @Serializable @SerialName("Ping")
    data class Ping(@SerialName("client_ms") val clientMs: Long) : ClientMsg()

    @Serializable @SerialName("Status")
    data class Status(val loading: Boolean, @SerialName("drift_ms") val driftMs: Int? = null) : ClientMsg()

    @Serializable @SerialName("Leave")
    data object Leave : ClientMsg()
}

@Serializable
sealed class ServerMsg {
    @Serializable @SerialName("Welcome")
    data class Welcome(val you: Member, @SerialName("room_state") val roomState: RoomState, @SerialName("server_ms") val serverMs: Long) : ServerMsg()

    @Serializable @SerialName("State")
    data class State(@SerialName("room_state") val roomState: RoomState) : ServerMsg()

    @Serializable @SerialName("ResolveRequest")
    data class ResolveRequest(@SerialName("request_id") val requestId: Long, val url: String) : ServerMsg()

    @Serializable @SerialName("Pong")
    data class Pong(@SerialName("client_ms") val clientMs: Long, @SerialName("server_ms") val serverMs: Long) : ServerMsg()

    @Serializable @SerialName("Members")
    data class Members(val list: List<Member>) : ServerMsg()

    @Serializable @SerialName("Error")
    data class Error(val message: String) : ServerMsg()

    @Serializable @SerialName("Rejected")
    data class Rejected(val message: String) : ServerMsg()

    @Serializable @SerialName("Toast")
    data class Toast(val message: String) : ServerMsg()
}

object Wire {
    val json = Json {
        classDiscriminator = "type"
        ignoreUnknownKeys = true
        explicitNulls = false
        encodeDefaults = true
    }

    fun encode(m: ClientMsg): String = json.encodeToString(ClientMsg.serializer(), m)

    /** Null for messages this client doesn't understand. */
    fun decode(s: String): ServerMsg? = try {
        json.decodeFromString(ServerMsg.serializer(), s)
    } catch (e: Exception) {
        null
    }
}
