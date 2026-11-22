package rs.synctube

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import kotlin.math.PI
import kotlin.math.pow
import kotlin.math.sin

class LoudnessTest {
    private fun sine(rate: Int, seconds: Int, dbfs: Double, channels: Int = 2): FloatArray {
        val a = 10.0.pow(dbfs / 20)
        val n = rate * seconds
        val out = FloatArray(n * channels)
        for (i in 0 until n) {
            val v = (a * sin(2 * PI * 1000.0 * i / rate)).toFloat()
            for (c in 0 until channels) out[i * channels + c] = v
        }
        return out
    }

    // EBU Tech 3341 case 1: stereo 1 kHz sine at -23 dBFS reads -23 LUFS.
    @Test fun ebuSineReadsMinus23() {
        for (rate in listOf(48_000, 44_100)) {
            val m = R128Meter(rate, 2)
            m.feed(sine(rate, 20, -23.0))
            assertEquals(-23.0, m.integrated()!!, 0.1)
            assertEquals(-23.0, m.peakDb()!!, 0.05)
        }
    }

    @Test fun fedInPiecesSameResult() {
        val all = sine(48_000, 10, -18.0)
        val m = R128Meter(48_000, 2)
        var i = 0
        while (i < all.size) {
            val n = minOf(1234 * 2, all.size - i)
            m.feed(all.copyOfRange(i, i + n))
            i += n
        }
        assertEquals(-18.0, m.integrated()!!, 0.1)
    }

    @Test fun silenceHasNoLoudness() {
        val m = R128Meter(48_000, 2)
        m.feed(FloatArray(48_000 * 2 * 5))
        assertNull(m.integrated())
    }

    @Test fun gainMatchesDesktop() {
        assertEquals(-4.0, LoudnessInfo(-6.0, -0.1).gainDb(), 1e-9)
        assertEquals(3.0, LoudnessInfo(-18.0, -4.0).gainDb(), 1e-9)
        assertEquals(8.0, LoudnessInfo(-18.0, null).gainDb(), 1e-9)
        assertEquals(0.0, LoudnessInfo(-18.0, 0.5).gainDb(), 1e-9)
        assertEquals(12.0, LoudnessInfo(-40.0, null).gainDb(), 1e-9)
        assertEquals(-24.0, LoudnessInfo(20.0, null).gainDb(), 1e-9)
        assertEquals(-2.0, LoudnessInfo.typicalGain(listOf(-5.0, -2.0, 1.0)), 1e-9)
        assertEquals(0.0, LoudnessInfo.typicalGain(emptyList()), 1e-9)
    }
}
