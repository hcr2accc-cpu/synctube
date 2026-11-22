package rs.synctube

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performScrollToIndex
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import okhttp3.OkHttpClient
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** Renders the phone screens to build/shots/ (gradle -Dsynctube.shots=1). */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w393dp-h851dp-xxhdpi", application = android.app.Application::class)
class ScreenshotTest {
    @get:Rule val compose = createComposeRule()

    @Before fun only() = assumeTrue(System.getProperty("synctube.shots") == "1")

    private fun engine(): Engine {
        val ctx = ApplicationProvider.getApplicationContext<android.app.Application>()
        return Engine(ctx, OkHttpClient(), Prefs(ctx))
    }

    private val queue = listOf(
        "Queen - Don't Stop Me Now (Official Video)" to 211_000L,
        "Daft Punk - One More Time" to 320_000L,
        "Arctic Monkeys - Do I Wanna Know? (Official Video)" to 272_000L,
        "Riblja Čorba - Lutka sa naslovne strane" to 245_000L,
        "Bajaga i Instruktori - Moji drugovi" to 263_000L,
        "Daft Punk - Harder, Better, Faster, Stronger" to 224_000L,
        "Tame Impala - The Less I Know The Better" to 216_000L,
        "Eminem - Lose Yourself" to 326_000L,
    ).mapIndexed { i, (t, d) -> Track("t$i", "id$i", t, d, if (i % 2 == 0) "Andrej" else "Marko") }

    private fun state(low: Boolean = false) = UiState(
        conn = ConnState.Connected,
        joined = true,
        room = RoomState(
            name = "ekipa",
            queue = queue,
            playback = PlaybackState(1, PlayStatus.Playing, 0, 0, 3),
        ),
        members = listOf(Member("a", "Andrej", MemberSync(driftMs = 0)), Member("b", "Marko", MemberSync(driftMs = 4))),
        rttMs = 36,
        clockSynced = true,
        phase = Phase.Playing,
        driftMs = 4,
        lowLatency = low,
    )

    private fun shot(name: String, content: @androidx.compose.runtime.Composable () -> Unit) {
        compose.setContent {
            SyncTubeTheme { Box(Modifier.fillMaxSize().background(Pal.BG)) { content() } }
        }
        compose.onRoot().captureRoboImage("build/shots/$name.png")
    }

    @Test fun room() {
        val e = engine()
        shot("room") { RoomScreen(e, state(low = true)) }
    }

    @Test fun roomScrolled() {
        val e = engine()
        compose.setContent {
            SyncTubeTheme { Box(Modifier.fillMaxSize().background(Pal.BG)) { RoomScreen(e, state()) } }
        }
        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(6)
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/shots/room_scrolled.png")
    }

    @Test fun connect() {
        val e = engine()
        shot("connect") { ConnectScreen(e, UiState()) }
    }

    @Test fun settings() {
        val e = engine()
        shot("settings") { SettingsDialog(e, state(low = true)) {} }
    }
}
