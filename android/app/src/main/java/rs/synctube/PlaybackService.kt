// Foreground service with the media session: lock screen / notification /
// headset controls. The controls go to the room (the server), not the local
// player, so everyone stays together.
package rs.synctube

import android.app.PendingIntent
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture

class PlaybackService : MediaSessionService() {
    private var session: MediaSession? = null
    private var player: SessionPlayer? = null

    override fun onCreate() {
        super.onCreate()
        val engine = App.instance.engine
        val p = SessionPlayer(engine)
        player = p
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        session = MediaSession.Builder(this, p).setSessionActivity(open).build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    @OptIn(UnstableApi::class)
    override fun onUpdateNotification(session: MediaSession, startInForegroundRequired: Boolean) {
        // Stay in the foreground for the whole time in a room, also while the
        // group is paused: playback can start again any moment from someone else.
        super.onUpdateNotification(session, startInForegroundRequired || App.instance.engine.inRoom)
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiped away: leave the room.
        App.instance.engine.leave()
        stopSelf()
    }

    override fun onDestroy() {
        player?.let { it.detach(); it.release() }
        session?.release()
        session = null
        super.onDestroy()
    }
}

/** What the session shows: the room's song and the group's play state. */
@OptIn(UnstableApi::class)
class SessionPlayer(private val engine: Engine) : SimpleBasePlayer(Looper.getMainLooper()) {
    private val handler = Handler(Looper.getMainLooper())
    private var pending: SettableFuture<Any?>? = null
    private var pendingRevision = -1L
    private val listener: () -> Unit = {
        val rev = engine.state.value.room?.revision ?: -1
        if (pending != null && rev != pendingRevision) finishPending()
        invalidateState()
    }

    init {
        engine.listeners.add(listener)
    }

    fun detach() {
        engine.listeners.remove(listener)
        finishPending()
    }

    private fun finishPending() {
        pending?.set(null)
        pending = null
    }

    /** Keeps the requested state shown until the server answers (no button flicker). */
    private fun untilServerAnswers(): ListenableFuture<*> {
        finishPending()
        val f = SettableFuture.create<Any?>()
        pending = f
        pendingRevision = engine.state.value.room?.revision ?: -1
        handler.postDelayed({ if (pending === f) finishPending() }, 1500)
        return f
    }

    override fun getState(): State {
        val s = engine.state.value
        val base = Player.Commands.Builder().addAll(
            COMMAND_PLAY_PAUSE, COMMAND_GET_CURRENT_MEDIA_ITEM, COMMAND_GET_METADATA, COMMAND_GET_TIMELINE,
        )
        if (!engine.inRoom) {
            return State.Builder()
                .setAvailableCommands(base.build())
                .setPlaybackState(STATE_IDLE)
                .setPlayWhenReady(false, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
                .build()
        }
        val r = s.room
        val idx = r?.playback?.trackIndex?.takeIf { r.currentTrack() != null }
        if (r == null || idx == null) {
            val meta = MediaMetadata.Builder()
                .setTitle(if (s.joined) S.NOTHING_PLAYING_TITLE else S.CONNECTING)
                .setArtist(r?.name ?: S.APP_TITLE)
                .build()
            val item = MediaItemData.Builder("idle")
                .setMediaItem(MediaItem.Builder().setMediaId("idle").setMediaMetadata(meta).build())
                .build()
            return State.Builder()
                .setAvailableCommands(base.build())
                .setPlaylist(listOf(item))
                .setCurrentMediaItemIndex(0)
                .setPlaybackState(STATE_READY)
                .setPlayWhenReady(false, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
                .build()
        }
        // The previous, current and next song, so the session offers skipping.
        val from = maxOf(0, idx - 1)
        val window = r.queue.subList(from, minOf(r.queue.size, idx + 2))
        val items = window.map { t ->
            val meta = MediaMetadata.Builder()
                .setTitle(t.title)
                .setArtist(if (t.addedBy.isNotEmpty()) "${r.name} · ${t.addedBy}" else r.name)
                .setArtworkUri(Uri.parse(YouTubeUrl.thumbUrl(t.youtubeId)))
                .build()
            MediaItemData.Builder(t.id)
                .setMediaItem(MediaItem.Builder().setMediaId(t.id).setMediaMetadata(meta).build())
                .setDurationUs(t.durationMs?.times(1000) ?: C.TIME_UNSET)
                .setIsSeekable(true)
                .build()
        }
        val playbackState = when (s.phase) {
            Phase.Resolving, Phase.Buffering -> STATE_BUFFERING
            else -> STATE_READY
        }
        base.addAll(
            COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM, COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
            COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM, COMMAND_SEEK_TO_MEDIA_ITEM,
        )
        return State.Builder()
            .setAvailableCommands(base.build())
            .setPlaylist(items)
            .setCurrentMediaItemIndex(idx - from)
            .setPlaybackState(playbackState)
            .setPlayWhenReady(s.isPlaying, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
            .setContentPositionMs { engine.positionMs() ?: 0 }
            .build()
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> {
        if (playWhenReady) engine.play() else engine.pause()
        return untilServerAnswers()
    }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> {
        when (seekCommand) {
            COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> engine.next()
            COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> engine.previous()
            COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM -> engine.seek(positionMs)
            COMMAND_SEEK_TO_MEDIA_ITEM -> {
                val r = engine.state.value.room
                val idx = r?.playback?.trackIndex
                if (r != null && idx != null) {
                    val target = maxOf(0, idx - 1) + mediaItemIndex
                    r.queue.getOrNull(target)?.let { engine.playTrack(it.id) }
                }
            }
            else -> return Futures.immediateVoidFuture()
        }
        return untilServerAnswers()
    }
}
