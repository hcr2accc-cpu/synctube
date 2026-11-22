package rs.synctube

import android.app.Application
import okhttp3.OkHttpClient
import java.net.InetAddress
import java.net.Socket
import java.util.concurrent.TimeUnit
import javax.net.SocketFactory

class App : Application() {
    lateinit var engine: Engine
        private set

    override fun onCreate() {
        super.onCreate()
        instance = this
        val http = OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .socketFactory(NoDelaySocketFactory())
            .build()
        engine = Engine(this, http, Prefs(this))
    }

    companion object {
        lateinit var instance: App
            private set
    }
}

/** Sends small frames (clock pings) at once instead of batching them. */
private class NoDelaySocketFactory : SocketFactory() {
    private val d = getDefault()
    private fun Socket.nd(): Socket = apply { tcpNoDelay = true }
    override fun createSocket(): Socket = d.createSocket().nd()
    override fun createSocket(host: String?, port: Int): Socket = d.createSocket(host, port).nd()
    override fun createSocket(host: String?, port: Int, la: InetAddress?, lp: Int): Socket =
        d.createSocket(host, port, la, lp).nd()
    override fun createSocket(host: InetAddress?, port: Int): Socket = d.createSocket(host, port).nd()
    override fun createSocket(host: InetAddress?, port: Int, la: InetAddress?, lp: Int): Socket =
        d.createSocket(host, port, la, lp).nd()
}
