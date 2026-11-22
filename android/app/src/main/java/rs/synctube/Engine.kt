// The client core: connection, clock sync, stream resolving and the sync loop
// that keeps the local player on the shared timeline. Mirrors client/src/engine.rs.
// Everything runs on the main thread; slow work hops to IO and comes back.
package rs.synctube

import android.content.Context
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import kotlin.math.abs

sealed class ConnState {
    data object Idle : ConnState()
    data class Connecting(val waking: Boolean) : ConnState()
    data object Connected : ConnState()
    data class Reconnecting(val retryInS: Long) : ConnState()
    data class Rejected(val reason: String) : ConnState()
    data class Failed(val reason: String) : ConnState()
}

sealed class Phase {
    data object Idle : Phase()
    data object Resolving : Phase()
    data object Buffering : Phase()
    /** Loaded and positioned, waiting for the scheduled start / other clients. */
    data object Waiting : Phase()
    data object Playing : Phase()
    data object Paused : Phase()
    data class Error(val message: String) : Phase()
}

data class PendingAdd(val id: Long, val playlist: Boolean)

data class SearchView(
    val query: String = "",
    val playlists: Boolean = false,
    val loading: Boolean = false,
    val results: List<SearchHit> = emptyList(),
    val error: String? = null,
) {
    val active: Boolean get() = query.isNotEmpty()
}

data class ToastMsg(val id: Long, val text: String, val error: Boolean)

data class UiState(
    val conn: ConnState = ConnState.Idle,
    val joined: Boolean = false,
    val room: RoomState? = null,
    val members: List<Member> = emptyList(),
    val me: Member? = null,
    val rttMs: Long? = null,
    val clockSynced: Boolean = false,
    val phase: Phase = Phase.Idle,
    val driftMs: Long? = null,
    val pendingAdds: List<PendingAdd> = emptyList(),
    val search: SearchView = SearchView(),
    val toasts: List<ToastMsg> = emptyList(),
    val volume: Int = 100,
    val shuffleLeft: Int? = null,
    val clearLeft: Int? = null,
    val focusLost: Boolean = false,
    val lowLatency: Boolean = false,
    val audioOffsetMs: Int = 0,
    val normalize: Boolean = true,
) {
    val isPlaying: Boolean get() = room?.playback?.status == PlayStatus.Playing
}

class Engine(private val ctx: Context, private val http: OkHttpClient, val prefs: Prefs) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val main = Handler(Looper.getMainLooper())
    private val _state = MutableStateFlow(
        UiState(volume = prefs.volume, lowLatency = prefs.lowLatency, audioOffsetMs = prefs.audioOffsetMs, normalize = prefs.normalize),
    )
    val state: StateFlow<UiState> = _state
    /** Called after every state change (the media session listens). */
    val listeners = ArrayList<() -> Unit>()

    val resolver = Resolver(http)
    private val clock = Clock().apply { lowLatency = prefs.lowLatency }
    private var lowLatency = prefs.lowLatency
    private var audioOffsetMs = prefs.audioOffsetMs
    private var normalize = prefs.normalize
    private val scanner = LoudnessScanner(http)
    private val loudness = prefs.loadLoudness()
    private val measureFailed = HashSet<String>()
    private var measureJob: Job? = null
    private var net: Net? = null
    private var player: AudioPlayer? = null

    private enum class CurPhase { Resolving, Loading, Seeking, Active, Failed }

    private class Cur(
        val trackId: String,
        val youtubeId: String,
        var durationMs: Long?,
        var phase: CurPhase = CurPhase.Resolving,
        var entry: Long = 0,
        var paused: Boolean = true,
        var speed: Double = 1.0,
        var reresolved: Boolean = false,
        var resolveRetried: Boolean = false,
    )

    private class AddJob(
        val id: Long,
        val url: String,
        val playlist: Boolean,
        var requestId: Long?,
        var result: Result<List<TrackInfo>>? = null,
        val started: Long = now(),
    )

    private class Shuffle(val order: List<String>, var done: Int = 0, var prev: String? = null, var sent: Pair<Long, Long>? = null)
    private class Clear(val ids: List<String>, var done: Int = 0, var sent: Pair<Long, Long>? = null)

    private var room: RoomState? = null
    private var cur: Cur? = null
    private var appliedVersion: Long? = null
    private var readySent: Long? = null
    private var endedSent: Long? = null
    private var unpauseAt: Long? = null
    private var unpauseJob: Job? = null
    private var driftJob: Job? = null
    private var pingJob: Job? = null
    private var stepJob: Job? = null
    private var alignAttempts = 0
    private var loadCompMs = 1500L
    private var seekCompMs = 300L
    private var opStarted: Long? = null
    private val adds = ArrayList<AddJob>()
    private var nextAddId = 0L
    private var searchSeq = 0L
    private var statusOk = false
    private var statusSent: Pair<MemberSync, Long>? = null
    private var shuffle: Shuffle? = null
    private var clear: Clear? = null

    private var conn: ConnState = ConnState.Idle
    private var joined = false
    private var members = emptyList<Member>()
    private var me: Member? = null
    private var driftMs: Long? = null
    private var search = SearchView()
    private val toasts = ArrayList<ToastMsg>()
    private var toastSeq = 0L
    private var volume = prefs.volume

    /** In a room (or trying to get in). */
    val inRoom: Boolean get() = net != null

    // ----- public commands (main thread) ---------------------------------------

    fun connect(url: String, roomName: String, password: String, name: String) {
        net?.close()
        net = null
        resetSession()
        val join = ClientMsg.Join(roomName, password.ifEmpty { null }, name)
        conn = ConnState.Connecting(false)
        lateinit var n: Net
        n = Net(url, join, scope) { ev -> scope.launch { if (net === n) { onNet(ev); publish() } } }
        net = n
        ensurePlayer()
        acquireLocks()
        requestFocus()
        publish()
    }

    fun leave() {
        net?.close()
        net = null
        resetSession()
        conn = ConnState.Idle
        releaseLocks()
        publish()
    }

    fun togglePlay() = user {
        val r = room
        if (r?.playback?.status == PlayStatus.Playing || r?.waitingForReady == true) send(ClientMsg.Pause) else send(ClientMsg.Play)
    }

    fun play() = user { send(ClientMsg.Play) }
    fun pause() = user { send(ClientMsg.Pause) }
    fun next() = user { send(ClientMsg.Next(room?.playback?.version)) }
    fun previous() = user { send(ClientMsg.Previous(room?.playback?.version)) }
    fun seek(ms: Long) = user { send(ClientMsg.Seek(ms.coerceAtLeast(0))) }
    fun playTrack(id: String) = user { send(ClientMsg.PlayTrack(id)) }
    fun remove(id: String) = user { send(ClientMsg.Remove(id)) }
    fun move(id: String, newIndex: Int) = user { send(ClientMsg.Move(id, newIndex.coerceAtLeast(0))) }

    /** Moves a song right after the current one. */
    fun playNext(id: String) = user {
        val r = room ?: return@user
        val idx = r.queue.indexOfFirst { it.id == id }
        if (idx < 0) return@user
        val target = (r.playback.trackIndex ?: -1) + 1
        val newIndex = if (idx < target) target - 1 else target
        if (newIndex != idx) send(ClientMsg.Move(id, newIndex))
    }

    /** A link is added; anything else is searched for. */
    fun submit(text: String) = user {
        val t = text.trim()
        if (t.isEmpty()) return@user
        val ref = YouTubeUrl.parse(t)
        if (ref != null) {
            val url = startAddJob(ref, null)
            send(ClientMsg.Add(url))
            search = SearchView().also { searchSeq++ }
        } else if (t.contains("://") || t.startsWith("www.")) {
            toast(S.NOT_YOUTUBE, true)
        } else {
            startSearch(t, search.playlists)
        }
    }

    fun search(query: String, playlists: Boolean) = user { startSearch(query, playlists) }

    fun addHit(h: SearchHit) = user {
        val ref = h.youtubeId?.let { YoutubeRef.Video(it) } ?: h.playlistId?.let { YoutubeRef.Playlist(it) } ?: return@user
        val url = startAddJob(ref, null)
        send(ClientMsg.Add(url))
    }

    fun clearSearch() = user {
        searchSeq++
        search = SearchView()
    }

    fun shuffleUpcoming() = user { startShuffle() }
    fun clearQueue() = user { startClear() }

    fun setVolume(v: Int) {
        volume = v.coerceIn(0, 100)
        prefs.volume = volume
        player?.setVolume(volume)
        publish()
    }

    /** Low-latency mode: constant pings, 5 drift checks a second, fixes from 3 ms. */
    fun setLowLatency(on: Boolean) {
        lowLatency = on
        prefs.lowLatency = on
        clock.lowLatency = on
        if (net != null) startPings()
        if (driftJob != null) scheduleDrift()
        if (!on) setSpeed(1.0)
        publish()
    }

    /** How late this device's sound comes out (Bluetooth); it plays that much ahead. */
    fun setAudioOffset(ms: Int) {
        audioOffsetMs = ms.coerceIn(-500, 500)
        prefs.audioOffsetMs = audioOffsetMs
        if (driftJob != null) scheduleDrift()
        publish()
    }

    /** Every song at about the same volume (one fixed gain per song). */
    fun setNormalize(on: Boolean) {
        normalize = on
        prefs.normalize = on
        applyGain()
        measureNext()
        publish()
    }

    fun dismissToast(id: Long) {
        toasts.removeAll { it.id == id }
        publish()
    }

    /** The app came to the front: take audio back if another app had it. */
    fun onAppVisible() {
        if (inRoom && !focusHeld) requestFocus()
        publish()
    }

    fun serverNow(): Long = clock.serverNow()

    /** Timeline position of the current track (what everyone hears now). */
    fun positionMs(): Long? {
        val r = room ?: return null
        val t = r.currentTrack() ?: return null
        return Sync.expectedPositionClamped(r.playback, serverNow(), t.durationMs)
    }

    private inline fun user(block: () -> Unit) {
        if (inRoom && !focusHeld) requestFocus()
        block()
        publish()
    }

    // ----- view -----------------------------------------------------------------

    private fun publish() {
        reportStatus()
        val t = now()
        adds.removeAll { it.requestId == null && t - it.started > ADD_ANSWER_TIMEOUT_MS }
        while (toasts.size > MAX_TOASTS) toasts.removeAt(0)
        _state.value = UiState(
            conn = conn,
            joined = joined,
            room = room,
            members = members,
            me = me,
            rttMs = clock.rttMs,
            clockSynced = clock.synced,
            phase = phase(),
            driftMs = driftMs,
            lowLatency = lowLatency,
            audioOffsetMs = audioOffsetMs,
            normalize = normalize,
            pendingAdds = adds.map { PendingAdd(it.id, it.playlist) },
            search = search,
            toasts = toasts.toList(),
            volume = volume,
            shuffleLeft = shuffle?.let { it.order.size - it.done },
            clearLeft = clear?.let { it.ids.size - it.done },
            focusLost = inRoom && !focusHeld,
        )
        schedulePaced()
        listeners.forEach { it() }
    }

    private fun phase(): Phase {
        val r = room ?: return Phase.Idle
        val c = cur ?: return Phase.Idle
        return when (c.phase) {
            CurPhase.Resolving -> Phase.Resolving
            CurPhase.Loading, CurPhase.Seeking -> Phase.Buffering
            CurPhase.Failed -> Phase.Error(S.RESOLVE_FAILED)
            CurPhase.Active -> when (r.playback.status) {
                PlayStatus.Playing -> if (unpauseAt != null || c.paused) Phase.Waiting else Phase.Playing
                PlayStatus.Paused -> if (r.waitingForReady) Phase.Waiting else Phase.Paused
                PlayStatus.Stopped -> Phase.Idle
            }
        }
    }

    /** Tells the room how this player is doing, when that changes. */
    private fun reportStatus() {
        if (!statusOk || !joined) return
        val ph = phase()
        val drift = if (ph == Phase.Playing) driftMs?.coerceIn(-10_000, 10_000)?.toInt() else null
        val nowSync = MemberSync(
            loading = ph == Phase.Resolving || ph == Phase.Buffering,
            driftMs = drift?.let {
                when {
                    lowLatency -> if (abs(it) <= Sync.LOW_DEADBAND_MS) 0 else it
                    abs(it) <= Sync.DRIFT_IGNORE_MS -> 0
                    else -> it / 20 * 20
                }
            },
        )
        statusSent?.let { (last, at) ->
            val sameKind = last.loading == nowSync.loading && (last.driftMs != null) == (nowSync.driftMs != null)
            if (last == nowSync || (sameKind && now() - at < STATUS_MIN_GAP_MS)) return
        }
        statusSent = nowSync to now()
        send(ClientMsg.Status(nowSync.loading, nowSync.driftMs))
    }

    private fun toast(text: String, error: Boolean) {
        Log.i(TAG, "toast: $text")
        toasts.add(ToastMsg(++toastSeq, text, error))
    }

    private fun send(m: ClientMsg) {
        net?.send(m)
    }

    private fun resetSession() {
        room = null
        joined = false
        members = emptyList()
        me = null
        adds.clear()
        shuffle = null
        clear = null
        clock.reset()
        pingJob?.cancel()
        pingJob = null
        unload()
    }

    // ----- network --------------------------------------------------------------

    private fun onNet(ev: NetEvent) {
        when (ev) {
            is NetEvent.Connecting -> conn = ConnState.Connecting(ev.waking)
            NetEvent.Connected -> {}
            is NetEvent.Disconnected -> {
                conn = ConnState.Reconnecting(ev.retryInS)
                pingJob?.cancel()
                pingJob = null
            }
            is NetEvent.Rejected -> {
                net = null
                resetSession()
                conn = ConnState.Rejected(ev.reason)
                releaseLocks()
            }
            is NetEvent.Failed -> {
                Log.w(TAG, "connection failed: ${ev.reason}")
                net = null
                resetSession()
                conn = ConnState.Failed(S.SERVER_UNREACHABLE)
                releaseLocks()
            }
            is NetEvent.Msg -> onServer(ev.msg, ev.atLocalMs)
        }
    }

    private fun onServer(m: ServerMsg, atLocal: Long) {
        when (m) {
            is ServerMsg.Welcome -> {
                conn = ConnState.Connected
                joined = true
                statusOk = m.you.sync != null
                statusSent = null
                me = m.you
                clock.setRough(m.serverMs)
                startPings()
                // Fresh state after (re)connect: re-apply everything.
                appliedVersion = null
                readySent = null
                room = m.roomState
                reconcile()
            }
            is ServerMsg.State -> {
                val old = room
                if (old != null && m.roomState.revision < old.revision) return
                cur?.let { c ->
                    if (c.durationMs == null) c.durationMs = m.roomState.queue.find { it.id == c.trackId }?.durationMs
                }
                room = m.roomState
                reconcile()
            }
            is ServerMsg.Members -> members = m.list
            is ServerMsg.Pong -> clock.addSample(m.clientMs, m.serverMs, atLocal)
            is ServerMsg.ResolveRequest -> {
                val job = adds.firstOrNull { it.url == m.url && it.requestId == null }
                if (job != null) {
                    job.requestId = m.requestId
                    tryFinishAdd(job)
                } else {
                    val ref = YouTubeUrl.parse(m.url)
                    if (ref != null) startAddJob(ref, m.requestId) else toast(S.NOT_YOUTUBE, true)
                }
            }
            is ServerMsg.Toast -> if (!m.message.contains(" je dodao/la")) toast(m.message, false)
            is ServerMsg.Error -> {
                adds.firstOrNull { it.requestId == null }?.let { adds.remove(it) }
                toast(m.message, true)
            }
            is ServerMsg.Rejected -> conn = ConnState.Rejected(m.message)
        }
    }

    private fun startPings() {
        pingJob?.cancel()
        pingJob = scope.launch {
            while (isActive) {
                if (lowLatency) {
                    send(ClientMsg.Ping(Clock.localMs()))
                    delay(LOW_PING_EVERY_MS)
                    continue
                }
                repeat(PING_BURST) {
                    send(ClientMsg.Ping(Clock.localMs()))
                    delay(PING_SPACING_MS)
                }
                delay(RESYNC_EVERY_MS)
            }
        }
    }

    // ----- adding links and search ---------------------------------------------------

    private fun startAddJob(r: YoutubeRef, requestId: Long?): String {
        val id = ++nextAddId
        val url = YouTubeUrl.canonical(r)
        adds.add(AddJob(id, url, r is YoutubeRef.Playlist, requestId))
        scope.launch {
            val res = runCatching { resolver.metadata(r) }
            res.exceptionOrNull()?.let { Log.w(TAG, "metadata failed for $url", it) }
            adds.firstOrNull { it.id == id }?.let {
                it.result = res
                tryFinishAdd(it)
            }
            publish()
        }
        return url
    }

    /** Sends AddResolved once both the server's request id and the metadata are known. */
    private fun tryFinishAdd(j: AddJob) {
        val rid = j.requestId ?: return
        val res = j.result ?: return
        adds.remove(j)
        res.fold(
            { tracks ->
                if (tracks.isEmpty()) toast(S.NOTHING_FOUND, true) else send(ClientMsg.AddResolved(rid, tracks))
            },
            { toast(S.RESOLVE_FAILED, true) },
        )
    }

    private fun startSearch(q: String, playlists: Boolean) {
        val query = q.trim()
        if (query.isEmpty()) return
        val seq = ++searchSeq
        search = SearchView(query = query, playlists = playlists, loading = true)
        scope.launch {
            val r = runCatching { resolver.search(query, playlists) }
            if (seq != searchSeq) return@launch
            r.exceptionOrNull()?.let { Log.w(TAG, "search failed", it) }
            search = r.fold(
                { if (it.isEmpty()) search.copy(loading = false, error = S.SEARCH_NOTHING) else search.copy(loading = false, results = it) },
                { search.copy(loading = false, error = S.SEARCH_FAILED) },
            )
            publish()
        }
    }

    // ----- shuffle / clear (one message at a time, paced) ----------------------------

    private fun startShuffle() {
        if (shuffle != null) {
            shuffle = null; return
        }
        if (clear != null) return
        val r = room ?: return
        val from = r.playback.trackIndex?.plus(1) ?: 0
        val order = r.queue.drop(from).map { it.id }.shuffled()
        if (order.size < 2) return
        shuffle = Shuffle(order)
    }

    private fun startClear() {
        if (clear != null) {
            clear = null; return
        }
        val r = room ?: return
        val current = r.currentTrack()?.id
        val ids = r.queue.reversed().map { it.id }.filter { it != current } + listOfNotNull(current)
        if (ids.isEmpty()) return
        shuffle = null
        clear = Clear(ids)
    }

    private fun paced(sent: Pair<Long, Long>?): Long {
        val (rev, at) = sent ?: return now()
        val answered = (room?.revision ?: 0) > rev
        return at + if (answered) SHUFFLE_GAP_MS else SHUFFLE_WAIT_MS
    }

    private fun schedulePaced() {
        stepJob?.cancel()
        stepJob = null
        val at = clear?.let { paced(it.sent) } ?: shuffle?.let { paced(it.sent) } ?: return
        stepJob = scope.launch {
            delay((at - now()).coerceAtLeast(0))
            stepJob = null
            if (clear != null) clearStep() else shuffleStep()
            publish()
        }
    }

    private fun clearStep() {
        val r = room
        val c = clear
        if (r == null || c == null) {
            clear = null; return
        }
        while (c.done < c.ids.size) {
            val id = c.ids[c.done++]
            if (r.queue.any { it.id == id }) {
                c.sent = r.revision to now()
                send(ClientMsg.Remove(id))
                return
            }
        }
        clear = null
        toast(S.QUEUE_CLEARED, false)
    }

    private fun shuffleStep() {
        val r = room
        val s = shuffle
        if (r == null || s == null) {
            shuffle = null; return
        }
        fun pos(id: String) = r.queue.indexOfFirst { it.id == id }.takeIf { it >= 0 }
        while (s.done < s.order.size) {
            val id = s.order[s.done++]
            val idx = pos(id) ?: continue
            val anchor = s.prev?.let(::pos) ?: r.playback.trackIndex
            s.prev = id
            val target = anchor?.plus(1) ?: 0
            if (idx == target) continue
            val newIndex = if (idx < target) target - 1 else target
            s.sent = r.revision to now()
            send(ClientMsg.Move(id, newIndex))
            return
        }
        shuffle = null
        toast(S.SHUFFLED, false)
    }

    // ----- player -------------------------------------------------------------------

    private fun ensurePlayer() {
        if (player != null) return
        player = AudioPlayer(ctx, http) { ev ->
            onPlayer(ev)
            publish()
        }.also {
            it.setVolume(volume)
            it.setFocusFactor(focusFactor)
        }
    }

    private fun unload() {
        if (cur != null) {
            player?.stop()
            player?.setSpeed(1.0)
            cur = null
        }
        appliedVersion = null
        cancelUnpause()
        driftJob?.cancel()
        driftJob = null
        driftMs = null
    }

    private fun onPlayer(ev: PlayerEvent) {
        when (ev) {
            PlayerEvent.Positioned -> onPositioned()
            is PlayerEvent.Eof -> {
                val c = cur ?: return
                if (c.entry != ev.entry || c.phase != CurPhase.Active) return
                room?.playback?.version?.let { v ->
                    if (endedSent != v) {
                        endedSent = v
                        send(ClientMsg.Ended(v))
                    }
                }
                driftJob?.cancel()
                driftJob = null
            }
            is PlayerEvent.Failed -> {
                val c = cur ?: return
                if (c.entry != ev.entry) return
                Log.w(TAG, "playback failed (${c.youtubeId}): ${ev.error}")
                if (!c.reresolved) {
                    // The cached URL may have expired: resolve again once.
                    c.reresolved = true
                    c.phase = CurPhase.Resolving
                    resolver.invalidate(c.youtubeId)
                    spawnResolve(c.trackId, c.youtubeId)
                } else {
                    giveUpTrack()
                }
            }
        }
    }

    /** Tells the group to move on from a track this client cannot play. */
    private fun giveUpTrack() {
        val title = room?.currentTrack()?.title ?: ""
        toast("${S.SKIPPING_BROKEN} $title", true)
        cur?.phase = CurPhase.Failed
        room?.playback?.version?.let { v ->
            if (endedSent != v) {
                endedSent = v
                send(ClientMsg.Ended(v))
            }
        }
    }

    private fun spawnResolve(trackId: String, youtubeId: String) {
        scope.launch {
            val r = runCatching { resolver.stream(youtubeId) }
            r.exceptionOrNull()?.let { Log.w(TAG, "resolve failed for $youtubeId", it) }
            onStream(trackId, r)
            publish()
        }
    }

    private fun onStream(trackId: String, result: Result<StreamUrl>) {
        val c = cur ?: return
        if (c.trackId != trackId || c.phase != CurPhase.Resolving) return
        val info = result.getOrElse {
            if (!c.resolveRetried) {
                c.resolveRetried = true
                resolver.invalidate(c.youtubeId)
                spawnResolve(c.trackId, c.youtubeId)
            } else {
                giveUpTrack()
            }
            return
        }
        if (c.durationMs == null && info.durationMs != null) {
            // Track was added without a duration: tell everyone.
            c.durationMs = info.durationMs
            send(ClientMsg.Duration(c.youtubeId, info.durationMs))
        }
        ensurePlayer()
        val r = room ?: return
        val pb = r.playback
        val nowS = serverNow()
        var start = if (pb.status == PlayStatus.Playing && nowS >= pb.anchorServerMs) {
            // Late joiner: aim where the timeline will be once loaded.
            pb.positionAt(nowS) + loadCompMs
        } else {
            pb.anchorPositionMs
        }
        c.durationMs?.let { start = minOf(start, (it - 1000).coerceAtLeast(0)) }
        val p = player ?: return
        c.entry = p.load(info.url, start)
        applyGain()
        c.phase = CurPhase.Loading
        c.paused = true
        c.speed = 1.0
        opStarted = now()
    }

    private fun onPositioned() {
        val c = cur ?: return
        val took = opStarted?.let { now() - it }
        opStarted = null
        when (c.phase) {
            CurPhase.Loading -> took?.let { loadCompMs = ewma(loadCompMs, it.coerceIn(200, 8000)) }
            CurPhase.Seeking -> took?.let { seekCompMs = ewma(seekCompMs, it.coerceIn(20, 5000)) }
            else -> return
        }
        c.phase = CurPhase.Active
        measureNext()
        continuePositioned()
    }

    /** After a load or seek finished: report ready or join the timeline. */
    private fun continuePositioned() {
        val version = room?.playback?.version
        if (appliedVersion != version) {
            reconcile(); return
        }
        when (room?.playback?.status) {
            PlayStatus.Paused -> sendReady()
            PlayStatus.Playing -> alignPlaying()
            else -> {}
        }
    }

    private fun sendReady() {
        val v = room?.playback?.version ?: return
        if (readySent != v) {
            readySent = v
            send(ClientMsg.Ready(v))
        }
    }

    // ----- sync -------------------------------------------------------------------------

    /** Brings the player in line with the room state. */
    private fun reconcile() {
        val r = room
        val track = r?.currentTrack()
        if (r == null || track == null || r.playback.status == PlayStatus.Stopped) {
            unload(); return
        }
        // The next song's URL is ready before it is needed.
        r.playback.trackIndex?.let { r.queue.getOrNull(it + 1) }?.let { next ->
            if (resolver.cachedStream(next.youtubeId) == null) scope.launch { resolver.prefetch(next.youtubeId) }
        }
        val c = cur
        if (c == null || c.trackId != track.id) {
            unload()
            endedSent = null
            readySent = null
            alignAttempts = 0
            cur = Cur(track.id, track.youtubeId, track.durationMs)
            spawnResolve(track.id, track.youtubeId)
            return
        }
        if (c.phase == CurPhase.Failed) {
            // Version changed but same broken track (e.g. seek): tell again.
            if (appliedVersion != r.playback.version) {
                appliedVersion = r.playback.version
                giveUpTrack()
            }
            return
        }
        if (c.phase != CurPhase.Active) return
        if (appliedVersion == r.playback.version) return
        appliedVersion = r.playback.version
        alignAttempts = 0
        cancelUnpause()
        when (r.playback.status) {
            PlayStatus.Paused -> {
                driftJob?.cancel()
                driftJob = null
                setPaused(true)
                setSpeed(1.0)
                val pos = player?.position()?.first
                val target = r.playback.anchorPositionMs
                if (pos == null || abs(pos - target) > 30) localSeek(target) else sendReady()
            }
            PlayStatus.Playing -> alignPlaying()
            PlayStatus.Stopped -> {}
        }
    }

    /** Waits (paused) if ahead, unpauses if slightly behind, seeks if far off. */
    private fun alignPlaying() {
        val r = room ?: return
        val pb = r.playback
        val (pos, atLocal) = player?.position() ?: return
        val serverAt = timelineAt(atLocal)
        val reach = pb.anchorServerMs + (pos - pb.anchorPositionMs)
        val wait = reach - serverAt
        val paused = cur?.paused ?: true
        // Within these the drift check fixes it with the speed (no pause/seek).
        val aheadOk = if (lowLatency) Sync.LOW_SEEK_MS else Sync.DRIFT_IGNORE_MS
        val behindOk = if (lowLatency) Sync.LOW_SEEK_MS else Sync.DRIFT_SEEK_MS
        if (wait in 0..MAX_WAIT_MS) {
            if (!paused) {
                if (wait <= aheadOk) {
                    scheduleDrift(); return
                }
                setPaused(true)
            }
            setSpeed(1.0)
            if (wait < 2) unpauseNow() else scheduleUnpause(atLocal + wait)
        } else if (wait < 0 && -wait <= behindOk) {
            if (paused) unpauseNow() else scheduleDrift()
        } else {
            alignAttempts++
            if (alignAttempts > 3) {
                Log.w(TAG, "cannot align after 3 seeks, playing anyway")
                unpauseNow(); return
            }
            val nowS = timelineAt(Clock.localMs())
            var target = if (nowS < pb.anchorServerMs) pb.anchorPositionMs else pb.positionAt(nowS) + seekCompMs
            cur?.durationMs?.let { target = minOf(target, (it - 500).coerceAtLeast(0)) }
            setPaused(true)
            setSpeed(1.0)
            localSeek(target)
        }
    }

    private fun scheduleUnpause(localTarget: Long) {
        cancelUnpause()
        unpauseAt = localTarget
        // Handler timing is far finer than the coroutine-dispatch path.
        val delayMs = (localTarget - Clock.localMs()).coerceAtLeast(0)
        val run = Runnable {
            unpauseNow()
            publish()
        }
        main.postAtTime(run, UNPAUSE_TOKEN, SystemClock.uptimeMillis() + delayMs)
    }

    private fun cancelUnpause() {
        unpauseAt = null
        main.removeCallbacksAndMessages(UNPAUSE_TOKEN)
        unpauseJob?.cancel()
        unpauseJob = null
    }

    private fun unpauseNow() {
        cancelUnpause()
        val playing = room?.playback?.status == PlayStatus.Playing
        val active = cur?.phase == CurPhase.Active
        if (playing && active) {
            setPaused(false)
            scheduleDrift()
        }
    }

    private fun scheduleDrift() {
        driftJob?.cancel()
        driftJob = scope.launch {
            delay(if (lowLatency) LOW_DRIFT_EVERY_MS else DRIFT_EVERY_MS)
            driftJob = null
            driftCheck()
            publish()
        }
    }

    /** The room's timeline at a local time, as this device should play it. */
    private fun timelineAt(atLocal: Long): Long =
        (Sync.toServerMs(atLocal, clock.offsetMs) + audioOffsetMs).coerceAtLeast(0)

    private fun driftCheck() {
        val r = room ?: return
        val pb = r.playback
        val c = cur
        val ok = pb.status == PlayStatus.Playing && unpauseAt == null && c != null && c.phase == CurPhase.Active && !c.paused
        if (!ok) return
        val (pos, atLocal) = player?.position() ?: run { scheduleDrift(); return }
        val serverAt = timelineAt(atLocal)
        if (serverAt < pb.anchorServerMs) {
            scheduleDrift(); return
        }
        val duration = c!!.durationMs
        val expected = Sync.expectedPositionClamped(pb, serverAt, duration)
        if (duration != null && expected + 1500 >= duration) return // near the end: let it finish
        val drift = pos - expected
        driftMs = drift
        when (val a = if (lowLatency) Sync.decideDriftLow(drift) else Sync.decideDrift(drift, c.speed)) {
            DriftAction.None -> scheduleDrift()
            is DriftAction.SetSpeed -> {
                setSpeed(a.speed); scheduleDrift()
            }
            DriftAction.Seek -> {
                Log.i(TAG, "drift $drift ms, re-aligning")
                alignAttempts = 0
                alignPlaying()
            }
        }
    }

    // ----- loudness ---------------------------------------------------------------------

    /** The current song's normalization gain (a typical one until it is measured). */
    private fun applyGain() {
        val c = cur
        val db = when {
            !normalize || c == null -> 0.0
            else -> loudness[c.youtubeId]?.gainDb() ?: LoudnessInfo.typicalGain(loudness.values.map { it.gainDb() })
        }
        player?.setGainDb(db)
    }

    /** Measures the current song and the next few, one at a time, once each. */
    private fun measureNext() {
        if (!normalize || measureJob != null) return
        val r = room ?: return
        val i = r.playback.trackIndex ?: return
        // Not while a song is starting: it gets the bandwidth first.
        if (cur?.phase != CurPhase.Active) return
        val id = r.queue.drop(i).take(MEASURE_AHEAD + 1).map { it.youtubeId }
            .firstOrNull { it !in loudness && it !in measureFailed } ?: return
        measureJob = scope.launch {
            val res = runCatching { scanner.measure(resolver.stream(id).url) }
            measureJob = null
            res.onSuccess {
                Log.i(TAG, "loudness $id: ${it.integratedLufs} LUFS, gain ${it.gainDb()} dB")
                loudness.remove(id)
                loudness[id] = it
                prefs.saveLoudness(loudness)
            }.onFailure {
                Log.w(TAG, "loudness scan failed for $id", it)
                measureFailed += id
            }
            if (cur?.youtubeId == id) applyGain()
            measureNext()
        }
    }

    private fun setPaused(paused: Boolean) {
        val c = cur ?: return
        val p = player ?: return
        if (c.paused != paused) {
            p.setPaused(paused)
            c.paused = paused
        }
    }

    private fun setSpeed(speed: Double) {
        val c = cur ?: return
        val p = player ?: return
        if (abs(c.speed - speed) > 1e-6) {
            p.setSpeed(speed)
            c.speed = speed
        }
    }

    private fun localSeek(posMs: Long) {
        val c = cur ?: return
        val p = player ?: return
        p.seek(posMs)
        c.phase = CurPhase.Seeking
        opStarted = now()
        driftJob?.cancel()
        driftJob = null
    }

    // ----- audio focus, wake and wifi locks ---------------------------------------------

    private val audio = ctx.getSystemService(AudioManager::class.java)
    private var focusRequest: AudioFocusRequest? = null
    private var focusHeld = false
    private var focusFactor = 1f

    private fun setFocus(held: Boolean, factor: Float) {
        focusHeld = held
        focusFactor = factor
        player?.setFocusFactor(factor)
    }

    private fun requestFocus() {
        val req = focusRequest ?: AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                android.media.AudioAttributes.Builder()
                    .setUsage(android.media.AudioAttributes.USAGE_MEDIA)
                    .setContentType(android.media.AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener({ change ->
                // Muted instead of paused, so the song stays in sync.
                when (change) {
                    AudioManager.AUDIOFOCUS_GAIN -> setFocus(true, 1f)
                    AudioManager.AUDIOFOCUS_LOSS -> setFocus(false, 0f)
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> setFocus(true, 0f)
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> setFocus(true, 0.3f)
                }
                publish()
            }, main)
            .build().also { focusRequest = it }
        val granted = audio.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        setFocus(granted, if (granted) 1f else 0f)
    }

    private fun abandonFocus() {
        focusRequest?.let { audio.abandonAudioFocusRequest(it) }
        setFocus(false, 1f)
    }

    private val wake = ctx.getSystemService(PowerManager::class.java)
        .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "synctube:room").apply { setReferenceCounted(false) }

    @Suppress("DEPRECATION")
    private val wifi = ctx.applicationContext.getSystemService(WifiManager::class.java)
        .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "synctube:room").apply { setReferenceCounted(false) }

    // Timers and the socket must keep running with the screen off while in a room.
    @android.annotation.SuppressLint("WakelockTimeout")
    private fun acquireLocks() {
        if (!wake.isHeld) wake.acquire()
        if (!wifi.isHeld) wifi.acquire()
    }

    private fun releaseLocks() {
        if (wake.isHeld) wake.release()
        if (wifi.isHeld) wifi.release()
        abandonFocus()
    }

    companion object {
        private const val TAG = "SyncTube"
        private const val PING_BURST = 8
        private const val PING_SPACING_MS = 100L
        private const val RESYNC_EVERY_MS = 30_000L
        private const val DRIFT_EVERY_MS = 1_000L
        private const val LOW_PING_EVERY_MS = 250L
        private const val MEASURE_AHEAD = 3
        private const val LOW_DRIFT_EVERY_MS = 200L
        private const val STATUS_MIN_GAP_MS = 400L
        private const val MAX_WAIT_MS = 6_000L
        private const val MAX_TOASTS = 20
        private const val ADD_ANSWER_TIMEOUT_MS = 20_000L
        private const val SHUFFLE_GAP_MS = 70L
        private const val SHUFFLE_WAIT_MS = 1_000L
        private val UNPAUSE_TOKEN = Any()

        private fun now() = SystemClock.elapsedRealtime()
        private fun ewma(old: Long, sample: Long) = (old * 3 + sample) / 4
    }
}
