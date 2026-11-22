package rs.synctube

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.getUnclippedBoundsInRoot
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.unit.Dp
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import okhttp3.OkHttpClient
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** Long-press a queue row and drag it: the other rows make room for it. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w393dp-h851dp-xxhdpi", application = android.app.Application::class)
class QueueDragTest {
    @get:Rule val compose = createComposeRule()

    private val queue = (0 until 8).map { Track("t$it", "id$it", "Pesma broj $it", 200_000L, "Andrej") }

    private fun top(text: String): Dp = compose.onNodeWithText(text).getUnclippedBoundsInRoot().top

    @Test fun dragUpMakesRoom() {
        val ctx = ApplicationProvider.getApplicationContext<android.app.Application>()
        val e = Engine(ctx, OkHttpClient(), Prefs(ctx))
        val s = UiState(
            conn = ConnState.Connected,
            joined = true,
            room = RoomState(name = "ekipa", queue = queue, playback = PlaybackState(0, PlayStatus.Paused, 0, 0, 1)),
            clockSynced = true,
        )
        compose.setContent {
            SyncTubeTheme { Box(Modifier.fillMaxSize().background(Pal.BG)) { RoomScreen(e, s) } }
        }
        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(4)
        compose.waitForIdle()

        val before1 = top("Pesma broj 1")
        val before3 = top("Pesma broj 3")
        val rowGap = before3 - top("Pesma broj 2")
        val list = compose.onNode(hasScrollToIndexAction())
        val listTop = list.getUnclippedBoundsInRoot().top
        val px = compose.density.density
        // Grab "Pesma broj 3" and drag it up past rows 2 and 1.
        val grab = Offset(200f * px, (before3 - listTop).value * px + 10f * px)
        list.performTouchInput {
            down(grab)
            advanceEventTime(viewConfiguration.longPressTimeoutMillis + 100)
            repeat(20) { moveBy(Offset(0f, -(rowGap.value * px * 2.1f) / 20f)) }
        }
        compose.waitForIdle()
        if (System.getProperty("synctube.shots") == "1") {
            compose.onRoot().captureRoboImage("build/shots/queue_drag.png")
        }
        val held1 = top("Pesma broj 1")
        assertTrue("row 1 should move down to make room ($before1 -> $held1)", held1 > before1 + rowGap / 2)

        list.performTouchInput { up() }
        compose.waitForIdle()
        // The local order stays until the server answers; row 3 now sits above row 1.
        assertTrue(top("Pesma broj 3") < top("Pesma broj 1"))
    }
}
