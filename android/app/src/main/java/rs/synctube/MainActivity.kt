package rs.synctube

import android.content.ComponentName
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.toArgb
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture

class MainActivity : ComponentActivity() {
    private var controller: ListenableFuture<MediaController>? = null
    /** A link shared from YouTube, added once the room is joined. */
    private val shared = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(Pal.BG.toArgb()),
            navigationBarStyle = SystemBarStyle.dark(Pal.BG.toArgb()),
        )
        if (savedInstanceState == null) takeShared(intent)
        val engine = App.instance.engine
        App.instance.updater.check()
        setContent {
            SyncTubeTheme {
                AppRoot(engine, shared.value) { shared.value = null }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeShared(intent)
    }

    private fun takeShared(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND) return
        val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
        // "Title https://youtu.be/..." -> the link part.
        shared.value = text.split(Regex("\\s+")).firstOrNull { YouTubeUrl.parse(it) != null } ?: text.trim()
    }

    override fun onStart() {
        super.onStart()
        // Binding a controller starts the playback service (and its session).
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        controller = MediaController.Builder(this, token).buildAsync()
    }

    override fun onResume() {
        super.onResume()
        App.instance.engine.onAppVisible()
    }

    override fun onStop() {
        controller?.let { MediaController.releaseFuture(it) }
        controller = null
        super.onStop()
    }
}
