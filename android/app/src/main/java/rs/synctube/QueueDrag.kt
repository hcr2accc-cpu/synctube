// Drag a queue song to a new place: long-press the row, then move it.
// A long press without moving opens the row's menu instead.
package rs.synctube

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.AnimationVector1D
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.lazy.LazyListItemInfo
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.first
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedback
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlin.math.abs

@Stable
class QueueDrag(private val list: LazyListState, private val scope: CoroutineScope) {
    /** Song being dragged, or settling into its place after release. */
    var id by mutableStateOf<String?>(null)
        private set

    /** Local order while dragging and until the server sends the new queue. */
    var order by mutableStateOf<List<String>?>(null)

    /** Song whose row menu is open. */
    var menu by mutableStateOf<String?>(null)

    /** Song ids on screen right now, in order (set on every composition). */
    var shown: List<String> = emptyList()
    var onMove: (id: String, newIndex: Int) -> Unit = { _, _ -> }

    // Dragged row's top edge, in the list's item-offset coordinates.
    private var top by mutableFloatStateOf(0f)
    private var settle by mutableStateOf<Animatable<Float, AnimationVector1D>?>(null)
    private var settleJob: Job? = null
    private var from = -1

    /** True while a finger is moving a row. */
    val active: Boolean get() = id != null && settle == null

    private fun info(key: Any?) = list.layoutInfo.visibleItemsInfo.firstOrNull { it.key == key }

    /** How far the row is drawn away from its slot in the list. */
    fun translation(key: String): Float {
        if (key != id) return 0f
        settle?.let { return it.value }
        val i = info(key) ?: return 0f
        return top - i.offset
    }

    /** Queue row under [y] (pointer position inside the list). */
    fun rowAt(y: Float): LazyListItemInfo? {
        val li = list.layoutInfo
        val at = y - li.beforeContentPadding
        return li.visibleItemsInfo.firstOrNull { it.key in shown && at >= it.offset && at < it.offset + it.size }
    }

    fun start(key: String) {
        val row = info(key) ?: return
        settleJob?.cancel()
        settle = null
        menu = null
        order = shown
        from = shown.indexOf(key)
        top = row.offset.toFloat()
        id = key
    }

    fun dragBy(dy: Float) {
        if (!active) return
        top += dy
        swapIfNeeded(dy > 0)
    }

    private fun swapIfNeeded(down: Boolean) {
        val key = id ?: return
        val cur = order ?: return
        val me = info(key) ?: return
        val items = list.layoutInfo.visibleItemsInfo.filter { it.key != key && it.key in cur }
        val target = if (down) {
            items.filter { it.index > me.index && top + me.size > it.offset + it.size / 2f }.maxByOrNull { it.index }
        } else {
            items.filter { it.index < me.index && top < it.offset + it.size / 2f }.minByOrNull { it.index }
        } ?: return
        // The list keeps its first visible item in place; moving that item would
        // scroll the whole list with it, so pin the current scroll position.
        val first = list.firstVisibleItemIndex
        if (me.index == first || target.index == first) {
            list.requestScrollToItem(first, list.firstVisibleItemScrollOffset)
        }
        val to = cur.indexOf(target.key)
        order = cur.toMutableList().apply { remove(key); add(to, key) }
    }

    /** Finger lifted: send the move (or open the menu) and glide the row into place. */
    fun end(openMenu: Boolean) {
        val key = id ?: return
        val to = order?.indexOf(key) ?: -1
        if (!openMenu && to >= 0 && to != from) onMove(key, to) else order = null
        if (openMenu) menu = key
        val a = Animatable(translation(key))
        settle = a
        settleJob = scope.launch {
            try {
                a.animateTo(0f, spring(stiffness = Spring.StiffnessMediumLow))
            } finally {
                if (settle === a) {
                    settle = null
                    id = null
                }
            }
        }
    }

    /** Scrolls the list while the row is held near its top or bottom edge. */
    suspend fun autoScroll() {
        while (active) {
            // Sleep until the row is near an edge (or the drag ends).
            snapshotFlow { if (active) edgeSpeed() else null }.first { it == null || it != 0f }
            if (!active) return
            withFrameNanos { }
            val v = edgeSpeed()
            if (v != 0f && list.scrollBy(v) == 0f) {
                // Already at the end of the list: wait for the row to move.
                snapshotFlow { top }.drop(1).first()
            } else if (v != 0f) {
                swapIfNeeded(v > 0)
            }
        }
    }

    private fun edgeSpeed(): Float {
        val me = info(id) ?: return 0f
        val li = list.layoutInfo
        val edge = me.size * 0.7f
        val startEdge = li.viewportStartOffset + edge
        val endEdge = li.viewportEndOffset - li.afterContentPadding - edge
        return when {
            top < startEdge -> -((startEdge - top) / edge).coerceAtMost(1f) * MAX_SCROLL
            top + me.size > endEdge -> ((top + me.size - endEdge) / edge).coerceAtMost(1f) * MAX_SCROLL
            else -> 0f
        }
    }

    private companion object {
        const val MAX_SCROLL = 26f // px per frame
    }
}

/** Long-press-and-drag gesture for the queue rows of a LazyColumn. */
fun Modifier.queueDrag(drag: QueueDrag, haptics: HapticFeedback): Modifier = pointerInput(drag) {
    awaitEachGesture {
        // Initial pass: the list sees the touch before the rows and the scroller,
        // so once a drag starts the rows don't get a click and the list doesn't scroll.
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val row = drag.rowAt(down.position.y) ?: return@awaitEachGesture
        val early = withTimeoutOrNull(viewConfiguration.longPressTimeoutMillis) {
            var gone = false
            while (!gone) {
                val c = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id }
                gone = c == null || !c.pressed ||
                    (c.position - down.position).getDistance() > viewConfiguration.touchSlop
            }
            true
        }
        if (early != null) return@awaitEachGesture

        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        drag.start(row.key as String)
        var moved = 0f
        var done = false
        try {
            while (true) {
                val c = awaitPointerEvent(PointerEventPass.Initial).changes.firstOrNull { it.id == down.id } ?: break
                val dy = c.positionChange().y
                c.consume()
                if (!c.pressed) break
                moved += abs(dy)
                if (dy != 0f) drag.dragBy(dy)
            }
            drag.end(openMenu = moved < viewConfiguration.touchSlop)
            done = true
        } finally {
            if (!done) drag.end(openMenu = false)
        }
    }
}
