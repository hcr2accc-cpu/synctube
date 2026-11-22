// Per-song loudness normalization, like the desktop app (client/src/loudness.rs):
// the start of each stream is decoded once in the background, measured
// (EBU R128 integrated loudness) and the song gets one fixed gain. Unlike a
// dynamic normalizer this never reshapes the sound; it only moves the level.
package rs.synctube

import android.media.AudioFormat
import android.media.MediaCodec
import android.media.MediaDataSource
import android.media.MediaExtractor
import android.media.MediaFormat
import android.os.SystemClock
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.pow
import kotlin.math.tan

data class LoudnessInfo(val integratedLufs: Double, val peakDb: Double?) {
    /** The fixed gain (dB) that brings this song to the target level. */
    fun gainDb(): Double {
        var g = TARGET_LUFS - integratedLufs
        if (g > 0 && peakDb != null) g = minOf(g, max(PEAK_CEILING_DB - peakDb, 0.0))
        return g.coerceIn(MAX_CUT_DB, MAX_BOOST_DB)
    }

    companion object {
        /** Same level as the desktop app, so phones and PCs match. */
        const val TARGET_LUFS = -10.0
        private const val PEAK_CEILING_DB = -1.0
        private const val MAX_BOOST_DB = 12.0
        private const val MAX_CUT_DB = -24.0

        /** Gain for a song not measured yet: the middle of the known ones. */
        fun typicalGain(gains: Collection<Double>): Double {
            if (gains.isEmpty()) return 0.0
            val s = gains.sorted()
            return s[s.size / 2]
        }
    }
}

/** EBU R128 / BS.1770 integrated loudness of interleaved float samples. */
class R128Meter(private val rate: Int, private val channels: Int) {
    private class Biquad(val b0: Double, val b1: Double, val b2: Double, val a1: Double, val a2: Double) {
        var z1 = 0.0
        var z2 = 0.0
        fun run(x: Double): Double {
            val y = b0 * x + z1
            z1 = b1 * x - a1 * y + z2
            z2 = b2 * x - a2 * y
            return y
        }
    }

    private val shelf = Array(channels) { highShelf() }
    private val highPass = Array(channels) { highPass() }
    private val step = rate / 10 // 100 ms
    private var stepFrames = 0
    private var stepEnergy = 0.0
    private val steps = ArrayList<Double>() // mean-square energy of each 100 ms
    private val blocks = ArrayList<Double>() // 400 ms blocks, 75 % overlap
    private var peak = 0.0
    var frames = 0L
        private set

    private fun highShelf(): Biquad {
        val f0 = 1681.974450955533
        val g = 3.999843853973347
        val q = 0.7071752369554196
        val k = tan(PI * f0 / rate)
        val vh = 10.0.pow(g / 20)
        val vb = vh.pow(0.4996667741545416)
        val a0 = 1 + k / q + k * k
        return Biquad(
            (vh + vb * k / q + k * k) / a0,
            2 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
            2 * (k * k - 1) / a0,
            (1 - k / q + k * k) / a0,
        )
    }

    private fun highPass(): Biquad {
        val f0 = 38.13547087602444
        val q = 0.5003270373238773
        val k = tan(PI * f0 / rate)
        val a0 = 1 + k / q + k * k
        return Biquad(1.0, -2.0, 1.0, 2 * (k * k - 1) / a0, (1 - k / q + k * k) / a0)
    }

    /** `samples` interleaved, `count` values (a whole number of frames). */
    fun feed(samples: FloatArray, count: Int = samples.size) {
        var i = 0
        while (i + channels <= count) {
            var e = 0.0
            for (c in 0 until channels) {
                val x = samples[i + c].toDouble()
                val a = abs(x)
                if (a > peak) peak = a
                // Surround channels (4, 5) count 1.41x; LFE is not in stereo music.
                val w = if (channels >= 5 && (c == 4 || c == 5)) 1.41 else 1.0
                val y = highPass[c].run(shelf[c].run(x))
                e += w * y * y
            }
            stepEnergy += e
            stepFrames++
            frames++
            if (stepFrames == step) endStep()
            i += channels
        }
    }

    private fun endStep() {
        steps.add(stepEnergy / stepFrames)
        stepEnergy = 0.0
        stepFrames = 0
        val n = steps.size
        if (n >= 4) blocks.add((steps[n - 1] + steps[n - 2] + steps[n - 3] + steps[n - 4]) / 4)
    }

    private fun lufs(power: Double) = -0.691 + 10 * log10(power)

    /** Gated integrated loudness, or null if (nearly) silent. */
    fun integrated(): Double? {
        val abs = blocks.filter { it > 0 && lufs(it) > -70.0 }
        if (abs.isEmpty()) return null
        val relGate = lufs(abs.average()) - 10.0
        val rel = abs.filter { lufs(it) > relGate }
        if (rel.isEmpty()) return null
        return lufs(rel.average())
    }

    fun peakDb(): Double? = if (peak > 0) 20 * log10(peak) else null
}

/** Decodes the start of a stream (fast, silently) and measures it. */
class LoudnessScanner(private val http: OkHttpClient) {
    suspend fun measure(url: String): LoudnessInfo = withContext(Dispatchers.IO) {
        val src = HttpSource(http, url)
        val ex = MediaExtractor()
        var codec: MediaCodec? = null
        try {
            ex.setDataSource(src)
            val track = (0 until ex.trackCount).firstOrNull {
                ex.getTrackFormat(it).getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true
            } ?: error("no audio track")
            ex.selectTrack(track)
            val fmt = ex.getTrackFormat(track)
            val c = MediaCodec.createDecoderByType(fmt.getString(MediaFormat.KEY_MIME)!!)
            codec = c
            c.configure(fmt, null, null, 0)
            c.start()
            decode(ex, c, fmt)
        } finally {
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            runCatching { ex.release() }
            src.close()
        }
    }

    private fun decode(ex: MediaExtractor, c: MediaCodec, inFmt: MediaFormat): LoudnessInfo {
        val started = SystemClock.elapsedRealtime()
        val info = MediaCodec.BufferInfo()
        var rate = inFmt.getInteger(MediaFormat.KEY_SAMPLE_RATE)
        var channels = inFmt.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
        var float = false
        var meter: R128Meter? = null
        var buf = FloatArray(0)
        var inputDone = false
        while (true) {
            if (SystemClock.elapsedRealtime() - started > SCAN_TIMEOUT_MS) error("scan timed out")
            if (!inputDone) {
                val i = c.dequeueInputBuffer(10_000)
                if (i >= 0) {
                    val b = c.getInputBuffer(i)!!
                    val n = ex.readSampleData(b, 0)
                    if (n < 0 || ex.sampleTime > SCAN_US) {
                        c.queueInputBuffer(i, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        inputDone = true
                    } else {
                        c.queueInputBuffer(i, 0, n, ex.sampleTime, 0)
                        ex.advance()
                    }
                }
            }
            val o = c.dequeueOutputBuffer(info, 10_000)
            when {
                o == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val f = c.outputFormat
                    rate = f.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                    channels = f.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                    float = f.containsKey(MediaFormat.KEY_PCM_ENCODING) &&
                        f.getInteger(MediaFormat.KEY_PCM_ENCODING) == AudioFormat.ENCODING_PCM_FLOAT
                    meter = null
                }
                o >= 0 -> {
                    val m = meter ?: R128Meter(rate, channels).also { meter = it }
                    val b = c.getOutputBuffer(o)!!.order(ByteOrder.nativeOrder())
                    b.position(info.offset)
                    b.limit(info.offset + info.size)
                    val count = if (float) info.size / 4 else info.size / 2
                    if (buf.size < count) buf = FloatArray(count)
                    if (float) {
                        b.asFloatBuffer().get(buf, 0, count)
                    } else {
                        val s = b.asShortBuffer()
                        for (k in 0 until count) buf[k] = s.get(k) / 32768f
                    }
                    m.feed(buf, count - count % channels)
                    c.releaseOutputBuffer(o, false)
                    if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
                }
            }
        }
        val m = meter ?: error("nothing decoded")
        if (m.frames < rate * 3L) error("too short")
        val lufs = m.integrated() ?: error("silent")
        return LoudnessInfo(lufs, m.peakDb())
    }

    /** Ranged HTTP reads in big chunks, with the headers YouTube wants. */
    private class HttpSource(private val http: OkHttpClient, private val url: String) : MediaDataSource() {
        private val headers = Resolver.streamHeaders(url)
        private var size = -1L
        private val chunks = LinkedHashMap<Long, ByteArray>()

        private fun chunk(index: Long): ByteArray? {
            chunks[index]?.let { return it }
            val from = index * CHUNK
            if (size in 0..from) return null
            val req = Request.Builder().url(url).apply {
                headers.forEach { (k, v) -> header(k, v) }
                header("Range", "bytes=$from-${from + CHUNK - 1}")
            }.build()
            http.newCall(req).execute().use { r ->
                if (r.code == 416) {
                    if (size < 0) size = from
                    return null
                }
                if (!r.isSuccessful) error("HTTP ${r.code}")
                if (size < 0) {
                    size = r.header("Content-Range")?.substringAfter('/')?.toLongOrNull()
                        ?: if (r.code == 200) (r.body?.contentLength() ?: -1) else -1
                }
                var data = r.body!!.bytes()
                if (r.code == 200 && from > 0) data = data.copyOfRange(from.toInt(), minOf(data.size, (from + CHUNK).toInt()))
                if (chunks.size >= 4) chunks.remove(chunks.keys.first())
                chunks[index] = data
                return data
            }
        }

        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (size == 0) return 0
            var done = 0
            while (done < size) {
                val p = position + done
                val c = chunk(p / CHUNK) ?: break
                val at = (p % CHUNK).toInt()
                if (at >= c.size) break
                val n = minOf(size - done, c.size - at)
                System.arraycopy(c, at, buffer, offset + done, n)
                done += n
                if (c.size < CHUNK) break
            }
            return if (done == 0) -1 else done
        }

        override fun getSize(): Long {
            if (size < 0) runCatching { chunk(0) }
            return size
        }

        override fun close() {
            chunks.clear()
        }
    }

    companion object {
        /** Only the start is scanned, so hour-long mixes stay quick. */
        private const val SCAN_US = 300_000_000L
        private const val SCAN_TIMEOUT_MS = 45_000L
        private const val CHUNK = 1L shl 20
    }
}

/** Applies the song's gain to the decoded audio, gliding to a new value. */
@OptIn(UnstableApi::class)
class GainProcessor : BaseAudioProcessor() {
    @Volatile private var target = 1.0f
    private var gain = 1.0f
    private var glide = 0f

    fun setGainDb(db: Double) {
        target = 10.0.pow(db / 20).toFloat()
    }

    override fun onConfigure(inputAudioFormat: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        val e = inputAudioFormat.encoding
        if (e != C.ENCODING_PCM_16BIT && e != C.ENCODING_PCM_FLOAT) {
            throw AudioProcessor.UnhandledAudioFormatException(inputAudioFormat)
        }
        // About 0.4 s to glide to a new level, so a late measurement is not a jump.
        glide = 1f / (0.4f * inputAudioFormat.sampleRate)
        return inputAudioFormat
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val n = inputBuffer.remaining()
        if (n == 0) return
        val out = replaceOutputBuffer(n)
        val fmt = inputAudioFormat
        val ch = fmt.channelCount
        val t = target
        if (fmt.encoding == C.ENCODING_PCM_FLOAT) {
            var i = 0
            while (inputBuffer.remaining() >= 4) {
                if (i % ch == 0) gain = step(gain, t)
                out.putFloat(inputBuffer.float * gain)
                i++
            }
        } else {
            var i = 0
            while (inputBuffer.remaining() >= 2) {
                if (i % ch == 0) gain = step(gain, t)
                val v = (inputBuffer.short * gain).toInt().coerceIn(-32768, 32767)
                out.putShort(v.toShort())
                i++
            }
        }
        out.flip()
    }

    private fun step(g: Float, t: Float): Float {
        if (g == t) return g
        val d = t - g
        val s = max(abs(d) * glide * 4f, glide * 0.05f)
        return if (abs(d) <= s) t else g + if (d > 0) s else -s
    }

    override fun onFlush() {
        gain = target
    }

    override fun onReset() {
        gain = target
    }
}
