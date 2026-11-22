// The local audio output: ExoPlayer with commands the engine needs (load at a
// position, seek, pause, speed) and a "positioned" event once it can play.
package rs.synctube

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.annotation.OptIn
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import okhttp3.OkHttpClient

sealed class PlayerEvent {
    /** A load or seek finished: the player can start right away. */
    data object Positioned : PlayerEvent()
    data class Eof(val entry: Long) : PlayerEvent()
    data class Failed(val entry: Long, val error: String) : PlayerEvent()
}

@OptIn(UnstableApi::class)
class AudioPlayer(context: Context, http: OkHttpClient, private val onEvent: (PlayerEvent) -> Unit) {
    private val handler = Handler(Looper.getMainLooper())
    private var entry = 0L
    private var pendingOp = false
    private var opAt = 0L
    private var volume = 1f
    private var focus = 1f

    private val exo: ExoPlayer

    init {
        val upstream = OkHttpDataSource.Factory(http)
        val source = ResolvingDataSource.Factory(upstream) { spec ->
            spec.withAdditionalHeaders(Resolver.streamHeaders(spec.uri.toString()))
        }
        val load = DefaultLoadControl.Builder()
            .setBufferDurationsMs(15_000, 60_000, 1_000, 2_000)
            .build()
        val attrs = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
        exo = ExoPlayer.Builder(context)
            .setMediaSourceFactory(DefaultMediaSourceFactory(source))
            .setLoadControl(load)
            // Focus is handled by the engine: it mutes instead of pausing, so
            // the song stays on the group's timeline.
            .setAudioAttributes(attrs, false)
            .setHandleAudioBecomingNoisy(false)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        exo.playWhenReady = false
        exo.addListener(object : Player.Listener {
            override fun onPlaybackStateChanged(state: Int) {
                when (state) {
                    Player.STATE_READY -> if (pendingOp) positioned()
                    Player.STATE_ENDED -> {
                        pendingOp = false
                        val e = entry
                        handler.post { onEvent(PlayerEvent.Eof(e)) }
                    }
                    else -> {}
                }
            }

            override fun onPlayerError(error: PlaybackException) {
                pendingOp = false
                val e = entry
                val msg = "${error.errorCodeName}: ${error.cause?.message ?: error.message}"
                handler.post { onEvent(PlayerEvent.Failed(e, msg)) }
            }
        })
    }

    private fun positioned() {
        pendingOp = false
        handler.post { onEvent(PlayerEvent.Positioned) }
    }

    /** A seek inside the buffer may never leave READY: check after a moment. */
    private fun watchOp() {
        opAt = SystemClock.elapsedRealtime()
        pendingOp = true
        val check = object : Runnable {
            override fun run() {
                if (!pendingOp) return
                if (exo.playbackState == Player.STATE_READY && SystemClock.elapsedRealtime() - opAt >= 300) {
                    positioned()
                } else {
                    handler.postDelayed(this, 50)
                }
            }
        }
        handler.postDelayed(check, 300)
    }

    /** Opens a stream paused at `startMs`. Returns the entry id for events. */
    fun load(url: String, startMs: Long): Long {
        entry++
        exo.playWhenReady = false
        exo.playbackParameters = PlaybackParameters(1f)
        exo.setMediaItem(MediaItem.Builder().setUri(url).setMediaId(entry.toString()).build(), startMs)
        exo.prepare()
        watchOp()
        return entry
    }

    fun seek(posMs: Long) {
        exo.seekTo(posMs)
        watchOp()
    }

    fun stop() {
        pendingOp = false
        exo.playWhenReady = false
        exo.stop()
        exo.clearMediaItems()
    }

    fun setPaused(paused: Boolean) {
        exo.playWhenReady = !paused
    }

    fun setSpeed(speed: Double) {
        exo.playbackParameters = PlaybackParameters(speed.toFloat())
    }

    /** 0..100, on a cubic curve like mpv's. */
    fun setVolume(percent: Int) {
        val v = percent.coerceIn(0, 100) / 100f
        volume = v * v * v
        exo.volume = volume * focus
    }

    /** 1 = normal, lower while another app has audio focus. */
    fun setFocusFactor(f: Float) {
        focus = f
        exo.volume = volume * focus
    }

    /** Current position and the local time it was read at. */
    fun position(): Pair<Long, Long>? {
        if (exo.playbackState == Player.STATE_IDLE || exo.currentMediaItem == null) return null
        return exo.currentPosition to Clock.localMs()
    }

    fun release() {
        exo.release()
    }
}
