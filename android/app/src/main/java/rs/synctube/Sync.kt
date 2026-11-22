// Pure synchronization math. Mirrors shared/src/sync.rs.
package rs.synctube

import kotlin.math.abs

data class ClockSample(val clientSendMs: Long, val serverMs: Long, val clientRecvMs: Long) {
    val rttMs: Long get() = (clientRecvMs - clientSendMs).coerceAtLeast(0)

    /** server - local, assuming a symmetric path. */
    val offsetMs: Long get() = serverMs - (clientSendMs + clientRecvMs) / 2
}

data class ClockEstimate(val offsetMs: Long, val rttMs: Long)

sealed class DriftAction {
    data object None : DriftAction()
    data class SetSpeed(val speed: Double) : DriftAction()
    data object Seek : DriftAction()
}

object Sync {
    const val DRIFT_IGNORE_MS = 50L
    const val DRIFT_SETTLED_MS = 20L
    const val DRIFT_SEEK_MS = 1000L
    const val SPEED_FAST = 1.03
    const val SPEED_SLOW = 0.97

    // Low-latency mode: correct from 3 ms, proportional speed, seek past 300 ms.
    const val LOW_DEADBAND_MS = 3L
    const val LOW_SEEK_MS = 300L
    const val LOW_CORRECT_MS = 1000.0
    const val LOW_MAX_DEVIATION = 0.04

    fun expectedPositionMs(s: PlaybackState, serverNowMs: Long): Long = when (s.status) {
        PlayStatus.Playing -> s.anchorPositionMs + (serverNowMs - s.anchorServerMs).coerceAtLeast(0)
        PlayStatus.Paused, PlayStatus.Stopped -> s.anchorPositionMs
    }

    fun expectedPositionClamped(s: PlaybackState, serverNowMs: Long, durationMs: Long?): Long {
        val p = expectedPositionMs(s, serverNowMs)
        return if (durationMs != null) minOf(p, durationMs) else p
    }

    /** Median offset of the best quarter (1..4) of samples by round trip time. */
    fun estimateClockOffset(samples: List<ClockSample>): ClockEstimate? {
        val valid = samples.filter { it.clientRecvMs >= it.clientSendMs }.sortedBy { it.rttMs }
        if (valid.isEmpty()) return null
        val take = (valid.size / 4).coerceIn(1, 4)
        val best = valid.take(take)
        val offsets = best.map { it.offsetMs }.sorted()
        val n = offsets.size
        val offset = if (n % 2 == 1) offsets[n / 2] else (offsets[n / 2 - 1] + offsets[n / 2]) / 2
        return ClockEstimate(offset, best[0].rttMs)
    }

    fun toServerMs(localMs: Long, offsetMs: Long): Long = (localMs + offsetMs).coerceAtLeast(0)
    fun toLocalMs(serverMs: Long, offsetMs: Long): Long = (serverMs - offsetMs).coerceAtLeast(0)

    /** `driftMs` is actual - expected (positive = this client is ahead). */
    fun decideDrift(driftMs: Long, currentSpeed: Double): DriftAction {
        val a = abs(driftMs)
        if (a > DRIFT_SEEK_MS) return DriftAction.Seek
        val nudging = abs(currentSpeed - 1.0) > 1e-6
        val wanted = if (driftMs > 0) SPEED_SLOW else SPEED_FAST
        if (nudging) {
            val wrongDirection = (currentSpeed > 1.0 && driftMs > 0) || (currentSpeed < 1.0 && driftMs < 0)
            if (a <= DRIFT_SETTLED_MS || wrongDirection) {
                return if (a >= DRIFT_IGNORE_MS) DriftAction.SetSpeed(wanted) else DriftAction.SetSpeed(1.0)
            }
            return if (abs(currentSpeed - wanted) > 1e-6) DriftAction.SetSpeed(wanted) else DriftAction.None
        }
        return if (a < DRIFT_IGNORE_MS) DriftAction.None else DriftAction.SetSpeed(wanted)
    }

    /** Low-latency mode: speed proportional to the drift, mirrors decide_drift_low. */
    fun decideDriftLow(driftMs: Long): DriftAction {
        val a = abs(driftMs)
        if (a > LOW_SEEK_MS) return DriftAction.Seek
        if (a <= LOW_DEADBAND_MS) return DriftAction.SetSpeed(1.0)
        val dev = (driftMs / LOW_CORRECT_MS).coerceIn(-LOW_MAX_DEVIATION, LOW_MAX_DEVIATION)
        return DriftAction.SetSpeed(Math.round((1.0 - dev) * 1000.0) / 1000.0)
    }
}

/** Monotonic local clock in epoch-range ms, plus the server offset estimate. */
class Clock {
    private val samples = ArrayList<ClockSample>()
    private var estimate: ClockEstimate? = null
    private var rough: Long? = null
    var lowLatency = false

    fun reset() {
        samples.clear(); estimate = null; rough = null
    }

    fun setRough(serverMs: Long) {
        rough = serverMs - localMs()
    }

    fun addSample(clientSendMs: Long, serverMs: Long, clientRecvMs: Long = localMs()) {
        samples.add(ClockSample(clientSendMs, serverMs, clientRecvMs))
        while (samples.size > (if (lowLatency) 40 else 16)) samples.removeAt(0)
        estimate = Sync.estimateClockOffset(samples)
    }

    val offsetMs: Long get() = estimate?.offsetMs ?: rough ?: 0
    val rttMs: Long? get() = estimate?.rttMs
    val synced: Boolean get() = estimate != null
    fun serverNow(): Long = Sync.toServerMs(localMs(), offsetMs)

    companion object {
        private val baseNanos = System.nanoTime()
        private val baseEpoch = System.currentTimeMillis()
        fun localMs(): Long = baseEpoch + (System.nanoTime() - baseNanos) / 1_000_000
    }
}
