package rs.synctube

import androidx.activity.ComponentActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.captureScreenRoboImage
import okhttp3.OkHttpClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/** The "Za tebe" tab. Pictures go to build/shots/ with -Dsynctube.shots=1. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w393dp-h851dp-xxhdpi", application = android.app.Application::class)
class LibraryUiTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @get:Rule val tmp = TemporaryFolder()

    private val titles = listOf(
        "Riblja Čorba - Lutka sa naslovne strane", "Bajaga i Instruktori - Moji drugovi",
        "Tame Impala - The Less I Know The Better", "Daft Punk - One More Time",
        "Arctic Monkeys - Do I Wanna Know? (Official Video)", "Eminem - Lose Yourself",
    )

    private fun id(n: Int) = "video%06d".format(n)
    private fun song(n: Int) = Song(id(n), titles[n % titles.size], 180_000L + n * 7000, "Kanal $n")
    private val queue = (20 until 26).map { Track("t$it", id(it), "U redu $it", 200_000L, if (it % 2 == 0) "Andrej" else "Marko") }

    private fun fill(dir: File, history: Boolean = true) {
        val l = Library.load(dir)
        val now = Library.unixNow()
        for ((i, name) in listOf("Žurka", "Vožnja kolima", "Mirno veče").withIndex()) {
            val p = l.create(name)!!
            l.add(p, (i * 10 until i * 10 + 4 + i).map(::song))
        }
        if (history) l.record(song(1), true, now)
        l.setRecs((40 until 52).map(::song), now)
        l.savePlaylists()
        l.saveHistory()
    }

    private val lib = LibraryUiState()
    private var search by mutableStateOf(SearchView())

    private fun show(dir: File): Engine {
        val ctx = ApplicationProvider.getApplicationContext<android.app.Application>()
        val e = Engine(ctx, OkHttpClient(), Prefs(ctx), dir)
        val s = UiState(
            conn = ConnState.Connected,
            joined = true,
            room = RoomState(name = "ekipa", queue = queue, playback = PlaybackState(0, PlayStatus.Playing, 0, 0, 1)),
            clockSynced = true,
            phase = Phase.Playing,
        )
        compose.setContent {
            val es by e.state.collectAsState()
            SyncTubeTheme {
                Box(Modifier.fillMaxSize().background(Pal.BG)) { RoomScreen(e, s.copy(library = es.library, search = search), lib) }
            }
        }
        return e
    }

    private fun list() = compose.onAllNodes(hasScrollToIndexAction()).onFirst()

    /**
     * Under Robolectric a text field inside a dialog (also a plain one) never
     * lets Compose become idle, so such a dialog is only opened and looked at.
     */
    private fun openDialog(name: String, open: () -> Unit) {
        compose.mainClock.autoAdvance = false
        open()
        compose.mainClock.advanceTimeBy(1000)
        shot(name)
    }

    private fun openTab() {
        compose.onNodeWithText(S.TAB_FOR_YOU).performClick()
        compose.waitForIdle()
    }

    @OptIn(ExperimentalRoborazziApi::class)
    private fun shot(name: String) {
        if (compose.mainClock.autoAdvance) compose.waitForIdle()
        if (System.getProperty("synctube.shots") == "1") captureScreenRoboImage("build/shots/$name.png")
    }

    private fun back() {
        compose.activityRule.scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
        compose.waitForIdle()
    }

    @Test fun tabShowsPlaylistsAndRecommendations() {
        show(tmp.root.also(::fill))
        list().performScrollToNode(hasText(S.TAB_FOR_YOU))
        shot("tabs_queue")
        openTab()
        list().performScrollToIndex(3)
        compose.onNodeWithText(S.MY_PLAYLISTS).assertIsDisplayed()
        compose.onNodeWithText("Vožnja kolima").assertIsDisplayed()
        shot("for_you")
        list().performScrollToNode(hasText(song(43).title))
        list().performScrollToNode(hasText(S.RECOMMENDED))
        shot("for_you_recs")
        compose.onNodeWithText(S.CLEAR_HISTORY).performClick()
        shot("clear_history")
        compose.onNodeWithText(S.DELETE).performClick()
        compose.waitForIdle()
        compose.onNodeWithText(S.RECS_EMPTY).assertExists()
        compose.onNodeWithText(S.CLEAR_HISTORY).assertDoesNotExist()
        list().performScrollToIndex(3)
        compose.onNodeWithText("Vožnja kolima").assertIsDisplayed()
    }

    @Test fun emptyTabExplains() {
        val e = show(tmp.root)
        list().performScrollToNode(hasText(S.TAB_FOR_YOU))
        openTab()
        list().performScrollToIndex(3)
        compose.onNodeWithText(S.NO_PLAYLISTS).assertExists()
        compose.onNodeWithText(S.RECS_EMPTY_HINT).assertExists()
        shot("for_you_empty")
        compose.runOnUiThread { e.createPlaylist("Nova") }
        compose.waitForIdle()
        compose.onNodeWithText("Nova").assertExists()
        compose.onNodeWithText(S.songs(0)).assertExists()
        openDialog("new_playlist") { compose.onNodeWithText(S.NEW_PLAYLIST).performClick() }
        assertEquals(LibraryDialog.Create, lib.dialog)
    }

    @Test fun backClosesAnOpenedPlaylist() {
        val e = show(tmp.root.also(::fill))
        list().performScrollToNode(hasText(S.TAB_FOR_YOU))
        openTab()
        list().performScrollToNode(hasText("Vožnja kolima"))
        compose.onNodeWithText("Vožnja kolima").performClick()
        compose.waitForIdle()
        list().performScrollToIndex(3)
        compose.onNodeWithText(S.ADD_ALL).assertIsDisplayed()
        shot("playlist")
        // A long press must not start dragging (that is for the room's queue).
        compose.onNodeWithText(titles[10 % titles.size]).performTouchInput { longClick() }
        compose.onAllNodesWithContentDescription(S.MORE).onFirst().performClick()
        shot("playlist_menu")
        compose.onNodeWithText(S.MOVE_DOWN).performClick()
        compose.waitForIdle()
        val p = e.state.value.library.playlists[1]
        assertEquals(listOf(id(11), id(10)), p.tracks.take(2).map { it.youtubeId })
        compose.onNodeWithContentDescription(S.DELETE_PLAYLIST).performClick()
        shot("delete_playlist")
        compose.onNodeWithText(S.CANCEL).performClick()
        compose.waitForIdle()

        // Search covers the playlist and leaves it open underneath.
        search = SearchView("nesto", error = S.SEARCH_NOTHING)
        compose.waitForIdle()
        compose.onNodeWithText(S.ADD_ALL).assertDoesNotExist()
        compose.onNodeWithText(S.TAB_FOR_YOU).assertDoesNotExist()
        back()
        search = SearchView()
        compose.waitForIdle()
        list().performScrollToNode(hasText(S.ADD_ALL))
        back()
        compose.onNodeWithText(S.ADD_ALL).assertDoesNotExist()
        list().performScrollToNode(hasText(S.RECOMMENDED))
        compose.onNodeWithText(S.RECOMMENDED).assertExists()
        list().performScrollToNode(hasText(S.QUEUE))
        compose.onNodeWithText(S.QUEUE).performClick()
        compose.waitForIdle()
        list().performScrollToNode(hasText("U redu 21"))
    }

    @Test fun savesTheCurrentSong() {
        show(tmp.root.also(::fill))
        openDialog("picker") { compose.onNodeWithContentDescription(S.SAVE_TO_PLAYLIST).performClick() }
        assertEquals(LibraryDialog.Pick(SaveItem.Songs(listOf(queue[0].toSong())), "U redu 20"), lib.dialog)
    }

    @Test fun savesFromTheQueueMenu() {
        show(tmp.root.also(::fill))
        list().performScrollToNode(hasText("U redu 22"))
        compose.onNodeWithText("U redu 22").performTouchInput { longClick() }
        shot("queue_menu")
        openDialog("picker_queue") { compose.onNodeWithText(S.SAVE_TO_PLAYLIST).performClick() }
        assertEquals(LibraryDialog.Pick(SaveItem.Songs(listOf(queue[2].toSong())), "U redu 22"), lib.dialog)
    }

    @Test fun pickerMarksAndPicks() {
        val l = Library.load(tmp.root.also(::fill))
        val picked = ArrayList<SaveTo>()
        val pick = LibraryDialog.Pick(SaveItem.Songs(listOf(song(1))), song(1).title)
        compose.setContent {
            SyncTubeTheme {
                Box(Modifier.background(Pal.CARD)) { PickContent(LibraryView(l.playlists), pick) { picked += it } }
            }
        }
        compose.onNodeWithContentDescription(S.ALREADY_SAVED).assertExists()
        compose.onNodeWithText("Žurka").performClick() // it is there already
        compose.onNodeWithText("Mirno veče").performClick()
        compose.onNode(hasSetTextAction()).performTextInput("Skroz nova")
        compose.onNodeWithText(S.CREATE).performClick()
        shot("picker_content")
        assertEquals(listOf(SaveTo.Existing(l.playlists[2].id), SaveTo.New("Skroz nova")), picked)
    }

    @Test fun searchRows() {
        val ctx = ApplicationProvider.getApplicationContext<android.app.Application>()
        val e = Engine(ctx, OkHttpClient(), Prefs(ctx), tmp.root.also(::fill))
        val hits = (1 until 6).map { SearchHit(id(it), null, titles[it], "Kanal", 200_000L, null, null) } +
            SearchHit(null, "PLabcdefghijk", "Balkan hitovi 2026, najbolje pesme za žurku", "Neko", null, 124, null)
        compose.setContent {
            SyncTubeTheme {
                Box(Modifier.fillMaxSize().background(Pal.BG)) {
                    RoomScreen(
                        e,
                        UiState(
                            conn = ConnState.Connected,
                            room = RoomState(name = "ekipa", queue = queue),
                            search = SearchView("balkan", results = hits),
                            library = e.state.value.library,
                        ),
                        lib,
                    )
                }
            }
        }
        list().performScrollToIndex(3)
        shot("search")
        list().performScrollToNode(hasText("Balkan hitovi 2026, najbolje pesme za žurku"))
        val buttons = compose.onAllNodesWithContentDescription(S.SAVE_TO_PLAYLIST)
        val last = buttons.fetchSemanticsNodes().size - 1
        openDialog("picker_youtube") { buttons[last].performClick() }
        assertEquals(LibraryDialog.Pick(SaveItem.YoutubePlaylist("PLabcdefghijk"), hits.last().title), lib.dialog)
    }

    @Test fun recommendationsFromTheQueueSaySo() {
        val l = Library.load(tmp.root)
        l.setRecs((40 until 44).map(::song), Library.unixNow())
        l.saveHistory()
        show(tmp.root)
        list().performScrollToNode(hasText(S.TAB_FOR_YOU))
        openTab()
        list().performScrollToNode(hasText(S.RECS_FROM_QUEUE))
    }
}
