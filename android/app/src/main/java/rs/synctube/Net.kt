// WebSocket connection with auto-reconnect and Render cold-start handling. Mirrors client/src/net.rs.
package rs.synctube

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

sealed class NetEvent {
    data class Connecting(val waking: Boolean) : NetEvent()
    data object Connected : NetEvent()
    data class Msg(val msg: ServerMsg, val atLocalMs: Long) : NetEvent()
    data class Disconnected(val retryInS: Long) : NetEvent()
    data class Rejected(val reason: String) : NetEvent()
    data class Failed(val reason: String) : NetEvent()
}

class Net(
    private val url: String,
    private val join: ClientMsg.Join,
    scope: CoroutineScope,
    private val onEvent: (NetEvent) -> Unit,
) {
    private val client = OkHttpClient.Builder()
        .connectTimeout(70, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .pingInterval(20, TimeUnit.SECONDS)
        .build()

    @Volatile private var ws: WebSocket? = null
    @Volatile private var stopped = false
    private val job: Job = scope.launch(Dispatchers.IO) { run() }

    fun send(m: ClientMsg) {
        ws?.send(Wire.encode(m))
    }

    fun close() {
        stopped = true
        ws?.let {
            it.send(Wire.encode(ClientMsg.Leave))
            it.close(1000, null)
        }
        ws = null
        job.cancel()
    }

    private suspend fun run() {
        val started = System.currentTimeMillis()
        var everConnected = false
        var attempt = 0
        var lastError = ""
        while (!stopped && kotlinx.coroutines.currentCoroutineContext().isActive) {
            val waking = !everConnected && System.currentTimeMillis() - started >= WAKING_AFTER_MS
            onEvent(NetEvent.Connecting(waking))
            val opened = CompletableDeferred<Boolean>()
            val ended = CompletableDeferred<Boolean>() // true = stop, false = reconnect
            val listener = object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    opened.complete(true)
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    val at = Clock.localMs()
                    when (val m = Wire.decode(text)) {
                        null -> {}
                        is ServerMsg.Rejected -> {
                            onEvent(NetEvent.Rejected(m.message))
                            ended.complete(true)
                            webSocket.close(1000, null)
                        }
                        else -> onEvent(NetEvent.Msg(m, at))
                    }
                }

                override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                    webSocket.close(1000, null)
                    ended.complete(false)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    ended.complete(false)
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    lastError = t.message ?: t.javaClass.simpleName
                    opened.complete(false)
                    ended.complete(false)
                }
            }
            val socket = client.newWebSocket(Request.Builder().url(url).build(), listener)
            // Render holds the upgrade request while it boots; tell the user after a few seconds.
            val wakingJob = if (!everConnected && !waking) {
                CoroutineScope(Dispatchers.IO).launch {
                    delay(WAKING_AFTER_MS); onEvent(NetEvent.Connecting(true))
                }
            } else null
            val ok = withTimeoutOrNull(CONNECT_TIMEOUT_MS) { opened.await() } ?: false
            wakingJob?.cancel()
            if (stopped) {
                socket.cancel(); return
            }
            if (ok) {
                everConnected = true
                attempt = 0
                ws = socket
                socket.send(Wire.encode(join))
                onEvent(NetEvent.Connected)
                val stop = ended.await()
                ws = null
                if (stop || stopped) return
            } else {
                socket.cancel()
                if (lastError.isEmpty()) lastError = "timeout"
            }
            if (!everConnected && System.currentTimeMillis() - started >= COLD_START_LIMIT_MS) {
                onEvent(NetEvent.Failed(lastError))
                return
            }
            val d = BACKOFF[minOf(attempt, BACKOFF.size - 1)]
            attempt++
            onEvent(NetEvent.Disconnected(d))
            delay(d * 1000)
        }
    }

    companion object {
        private val BACKOFF = longArrayOf(1, 2, 5, 10)
        private const val COLD_START_LIMIT_MS = 90_000L
        private const val WAKING_AFTER_MS = 3_000L
        private const val CONNECT_TIMEOUT_MS = 70_000L
    }
}
