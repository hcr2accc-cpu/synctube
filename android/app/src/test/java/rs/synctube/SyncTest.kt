package rs.synctube

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SyncTest {
    @Test fun lowLatencyCorrectsSmallDrift() {
        assertEquals(DriftAction.SetSpeed(1.0), Sync.decideDriftLow(2))
        assertEquals(DriftAction.SetSpeed(1.01), Sync.decideDriftLow(-10))
        assertEquals(DriftAction.SetSpeed(0.98), Sync.decideDriftLow(20))
        assertEquals(DriftAction.SetSpeed(1.04), Sync.decideDriftLow(-250))
        assertEquals(DriftAction.SetSpeed(0.96), Sync.decideDriftLow(250))
        assertEquals(DriftAction.Seek, Sync.decideDriftLow(301))
        assertEquals(DriftAction.Seek, Sync.decideDriftLow(-301))
    }

    private fun playing(anchorServer: Long, anchorPos: Long) =
        PlaybackState(0, PlayStatus.Playing, anchorServer, anchorPos, 1)

    @Test fun expectedPositionPlayingAdvances() {
        val s = playing(10_000, 5_000)
        assertEquals(5_000, Sync.expectedPositionMs(s, 10_000))
        assertEquals(7_500, Sync.expectedPositionMs(s, 12_500))
    }

    @Test fun scheduledStartHolds() {
        val s = playing(10_000, 0)
        assertEquals(0, Sync.expectedPositionMs(s, 9_500))
        assertEquals(250, Sync.expectedPositionMs(s, 10_250))
    }

    @Test fun pausedIsAnchor() {
        val s = playing(10_000, 5_000).copy(status = PlayStatus.Paused)
        assertEquals(5_000, Sync.expectedPositionMs(s, 99_000))
    }

    @Test fun clampsToDuration() {
        assertEquals(3_000, Sync.expectedPositionClamped(playing(0, 0), 5_000, 3_000))
        assertEquals(5_000, Sync.expectedPositionClamped(playing(0, 0), 5_000, null))
    }

    @Test fun sampleOffsetSymmetric() {
        val s = ClockSample(1000, 5050, 1100)
        assertEquals(100, s.rttMs)
        assertEquals(4000, s.offsetMs)
    }

    @Test fun offsetPrefersLowRtt() {
        val samples = (0 until 6).map { i ->
            val send = 1000L + i * 1000
            ClockSample(send, send + 2010, send + 20)
        } + listOf(ClockSample(9000, 11_010, 9600), ClockSample(10_000, 12_010, 10_900))
        val est = Sync.estimateClockOffset(samples)!!
        assertEquals(2000, est.offsetMs)
        assertEquals(20, est.rttMs)
    }

    @Test fun offsetNegativeAndEmpty() {
        assertNull(Sync.estimateClockOffset(emptyList()))
        val est = Sync.estimateClockOffset(listOf(ClockSample(10_000, 4_010, 10_020)))!!
        assertEquals(-6000, est.offsetMs)
        assertEquals(4_000, Sync.toServerMs(10_000, est.offsetMs))
        assertEquals(10_000, Sync.toLocalMs(4_000, est.offsetMs))
        assertNull(Sync.estimateClockOffset(listOf(ClockSample(100, 0, 50))))
    }

    @Test fun drift() {
        assertEquals(DriftAction.None, Sync.decideDrift(0, 1.0))
        assertEquals(DriftAction.None, Sync.decideDrift(49, 1.0))
        assertEquals(DriftAction.SetSpeed(Sync.SPEED_SLOW), Sync.decideDrift(200, 1.0))
        assertEquals(DriftAction.SetSpeed(Sync.SPEED_FAST), Sync.decideDrift(-200, 1.0))
        assertEquals(DriftAction.SetSpeed(Sync.SPEED_SLOW), Sync.decideDrift(1000, 1.0))
        assertEquals(DriftAction.SetSpeed(Sync.SPEED_FAST), Sync.decideDrift(-50, 1.0))
        assertEquals(DriftAction.Seek, Sync.decideDrift(1001, 1.0))
        assertEquals(DriftAction.Seek, Sync.decideDrift(-5000, Sync.SPEED_FAST))
        assertEquals(DriftAction.None, Sync.decideDrift(-40, Sync.SPEED_FAST))
        assertEquals(DriftAction.SetSpeed(1.0), Sync.decideDrift(-15, Sync.SPEED_FAST))
        assertEquals(DriftAction.SetSpeed(1.0), Sync.decideDrift(18, Sync.SPEED_SLOW))
        assertEquals(DriftAction.SetSpeed(1.0), Sync.decideDrift(30, Sync.SPEED_FAST))
        assertEquals(DriftAction.SetSpeed(Sync.SPEED_SLOW), Sync.decideDrift(300, Sync.SPEED_FAST))
    }
}
