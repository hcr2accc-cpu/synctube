// Compose UI: connect screen and room screen.
package rs.synctube

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.VisibilityThreshold
import androidx.compose.animation.core.spring
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.Logout
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.PlaylistPlay
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.automirrored.rounded.VolumeUp
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ContentPaste
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.DeleteSweep
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Group
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Radio
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Shuffle
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material.icons.rounded.SkipPrevious
import androidx.compose.material.icons.rounded.VolumeOff
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.SliderColors
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.zIndex
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.LocalContext
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil.compose.AsyncImage
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs

internal val CardShape = RoundedCornerShape(14.dp)

@Composable
fun AppRoot(engine: Engine, shared: String?, onSharedTaken: () -> Unit) {
    val s by engine.state.collectAsStateWithLifecycle()
    LaunchedEffect(s.joined, shared) {
        if (s.joined && shared != null) {
            engine.submit(shared)
            onSharedTaken()
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(Pal.BG)
            .safeDrawingPadding(),
    ) {
        if (s.room != null) RoomScreen(engine, s) else ConnectScreen(engine, s)
        Toasts(
            engine,
            s.toasts,
            Modifier.align(Alignment.BottomCenter).padding(bottom = if (s.room != null) 76.dp else 0.dp),
        )
    }
}

// ----- connect ------------------------------------------------------------------

@Composable
internal fun ConnectScreen(engine: Engine, s: UiState) {
    val prefs = engine.prefs
    var server by rememberSaveable { mutableStateOf(prefs.serverUrl) }
    var room by rememberSaveable { mutableStateOf(prefs.room) }
    var nick by rememberSaveable { mutableStateOf(prefs.nickname) }
    var password by rememberSaveable { mutableStateOf(prefs.password) }
    var advanced by rememberSaveable { mutableStateOf(prefs.serverUrl.isBlank()) }
    var error by remember { mutableStateOf<String?>(null) }
    val busy = engine.inRoom
    val focus = LocalFocusManager.current

    fun go() {
        focus.clearFocus()
        val u = server.trim()
        val r = room.trim()
        val n = nick.trim()
        error = when {
            u.isEmpty() || r.isEmpty() || n.isEmpty() -> { if (u.isEmpty()) advanced = true; S.FILL_ALL }
            !(u.startsWith("ws://") || u.startsWith("wss://")) -> { advanced = true; S.BAD_SERVER_URL }
            else -> null
        }
        if (error != null) return
        prefs.serverUrl = u
        prefs.room = r
        prefs.nickname = n
        prefs.password = password
        prefs.rememberRoom(r)
        engine.connect(u, r, password, n)
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 28.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Logo()
        Spacer(Modifier.height(14.dp))
        Text(S.APP_TITLE, fontSize = 28.sp, fontWeight = FontWeight.Bold, color = Pal.TEXT)
        Text(S.CONNECT_HEADING, color = Pal.TEXT_DIM, fontSize = 15.sp)
        Spacer(Modifier.height(26.dp))
        Column(
            Modifier
                .widthIn(max = 480.dp)
                .fillMaxWidth()
                .clip(CardShape)
                .background(Pal.PANEL)
                .border(1.dp, Pal.BORDER, CardShape)
                .padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            UpdateBar()
            Field(room, { room = it }, S.ROOM, S.ROOM_HINT, enabled = !busy)
            val recent = prefs.recentRooms.filter { !it.equals(room.trim(), ignoreCase = true) }
            if (recent.isNotEmpty() && !busy) {
                RecentRooms(recent) { room = it }
            }
            Field(nick, { nick = it }, S.NICKNAME, S.NICK_HINT, enabled = !busy)
            Field(
                password, { password = it }, S.PASSWORD, null, enabled = !busy, password = true,
                ime = ImeAction.Go, onIme = { go() },
            )
            Row(
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .clickable { advanced = !advanced }
                    .padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(if (advanced) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, null, tint = Pal.TEXT_DIM)
                Text(S.ADVANCED, color = Pal.TEXT_DIM, fontSize = 14.sp)
            }
            if (advanced) {
                Field(server, { server = it }, S.SERVER_URL, S.SERVER_URL_HINT, enabled = !busy, keyboard = KeyboardType.Uri)
            }
            Spacer(Modifier.height(2.dp))
            if (busy) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = Pal.ACCENT)
                    Spacer(Modifier.width(10.dp))
                    Text(connText(s.conn), color = Pal.TEXT_DIM, fontSize = 14.sp, modifier = Modifier.weight(1f))
                }
                OutlinedButton(onClick = { engine.leave() }, modifier = Modifier.fillMaxWidth().height(50.dp)) {
                    Text(S.CANCEL)
                }
            } else {
                val shown = error ?: when (val c = s.conn) {
                    is ConnState.Rejected -> c.reason
                    is ConnState.Failed -> c.reason
                    else -> null
                }
                if (shown != null) Text(shown, color = Pal.BAD, fontSize = 14.sp)
                Button(
                    onClick = { go() },
                    modifier = Modifier.fillMaxWidth().height(50.dp),
                    shape = RoundedCornerShape(12.dp),
                ) {
                    Text(S.CONNECT, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                }
            }
        }
        Spacer(Modifier.height(24.dp))
        Text(S.ABOUT, color = Pal.TEXT_FAINT, fontSize = 12.sp)
    }
}

/** "New version" bar above the connect form: one tap downloads it and opens the installer. */
@Composable
private fun UpdateBar() {
    val updater = (LocalContext.current.applicationContext as? App)
        ?.takeIf { runCatching { it.updater }.isSuccess }?.updater ?: return
    val phase by updater.phase.collectAsState()
    val uri = LocalUriHandler.current
    @Composable
    fun row(text: String, button: (() -> Unit)?) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(text, color = Pal.TEXT, fontSize = 14.sp, modifier = Modifier.weight(1f))
            if (button != null) {
                Spacer(Modifier.width(10.dp))
                Button(onClick = button, shape = RoundedCornerShape(12.dp)) { Text(S.UPDATE_BUTTON) }
            }
        }
    }
    when (val p = phase) {
        UpdatePhase.Checking, UpdatePhase.UpToDate -> return
        is UpdatePhase.Available -> row(S.updateAvailable(p.update.tag)) { updater.install(p.update) }
        is UpdatePhase.NeedsPermission -> row(S.UPDATE_PERMISSION) { updater.install(p.update) }
        is UpdatePhase.Downloading -> Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            val frac = p.total?.takeIf { it > 0 }?.let { p.done.toFloat() / it }
            Text(S.updateDownloading(p.tag, frac?.let { (it * 100).toInt() }), color = Pal.TEXT_DIM, fontSize = 14.sp)
            if (frac != null) {
                LinearProgressIndicator(progress = { frac }, color = Pal.ACCENT, modifier = Modifier.fillMaxWidth())
            } else {
                LinearProgressIndicator(color = Pal.ACCENT, modifier = Modifier.fillMaxWidth())
            }
        }
        is UpdatePhase.Failed -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(S.updateFailed(p.message), color = Pal.BAD, fontSize = 14.sp)
            Text(
                S.UPDATE_MANUAL,
                color = Pal.ACCENT,
                fontSize = 14.sp,
                modifier = Modifier.clickable { uri.openUri(Updater.RELEASES_PAGE) },
            )
        }
    }
    HorizontalDivider(color = Pal.BORDER)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RecentRooms(rooms: List<String>, onPick: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(S.RECENT_ROOMS, color = Pal.TEXT_FAINT, fontSize = 12.sp)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            rooms.forEach { r ->
                Text(
                    r,
                    color = Pal.TEXT,
                    fontSize = 13.sp,
                    modifier = Modifier
                        .clip(RoundedCornerShape(50))
                        .background(Pal.CARD)
                        .border(1.dp, Pal.BORDER, RoundedCornerShape(50))
                        .clickable { onPick(r) }
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                )
            }
        }
    }
}

@Composable
private fun Logo() {
    Box(
        Modifier
            .size(72.dp)
            .clip(RoundedCornerShape(20.dp))
            .background(Pal.ACCENT.copy(alpha = 0.16f))
            .border(1.dp, Pal.ACCENT.copy(alpha = 0.4f), RoundedCornerShape(20.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(Icons.Rounded.MusicNote, null, tint = Pal.ACCENT, modifier = Modifier.size(38.dp))
    }
}

@Composable
private fun Field(
    value: String,
    onChange: (String) -> Unit,
    label: String,
    hint: String?,
    enabled: Boolean = true,
    password: Boolean = false,
    keyboard: KeyboardType = KeyboardType.Text,
    ime: ImeAction = ImeAction.Next,
    onIme: (() -> Unit)? = null,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        placeholder = hint?.let { { Text(it, color = Pal.TEXT_FAINT) } },
        singleLine = true,
        enabled = enabled,
        visualTransformation = if (password) PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
        keyboardOptions = KeyboardOptions(
            keyboardType = if (password) KeyboardType.Password else keyboard,
            imeAction = ime,
        ),
        keyboardActions = KeyboardActions(onGo = { onIme?.invoke() }),
        shape = RoundedCornerShape(10.dp),
        colors = fieldColors(),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
internal fun fieldColors() = OutlinedTextFieldDefaults.colors(
    focusedContainerColor = Pal.FIELD,
    unfocusedContainerColor = Pal.FIELD,
    disabledContainerColor = Pal.FIELD,
    focusedBorderColor = Pal.ACCENT,
    unfocusedBorderColor = Pal.BORDER,
    focusedLabelColor = Pal.ACCENT,
    unfocusedLabelColor = Pal.TEXT_DIM,
    cursorColor = Pal.ACCENT,
)

private fun connText(c: ConnState) = when (c) {
    is ConnState.Connecting -> if (c.waking) S.WAKING_SERVER else S.CONNECTING
    is ConnState.Reconnecting -> S.reconnectingIn(c.retryInS)
    ConnState.Connected -> S.CONNECTED
    is ConnState.Rejected -> c.reason
    is ConnState.Failed -> c.reason
    ConnState.Idle -> ""
}

// ----- room ---------------------------------------------------------------------

@Composable
internal fun RoomScreen(
    engine: Engine,
    s: UiState,
    lib: LibraryUiState = rememberSaveable(saver = LibraryUiState.Saver) { LibraryUiState() },
) {
    val room = s.room ?: return
    var showMembers by remember { mutableStateOf(false) }
    var confirmClear by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    // The big player is item 1..3 (after the top bar and banners): once it is
    // scrolled away, a small one stays at the bottom.
    val nowIndex = 1 + (if (s.conn !is ConnState.Connected) 1 else 0) + (if (s.focusLost) 1 else 0)
    val showMini by remember(nowIndex) {
        derivedStateOf {
            list.firstVisibleItemIndex > nowIndex ||
                (list.firstVisibleItemIndex == nowIndex && list.firstVisibleItemScrollOffset > 420)
        }
    }
    val opened = s.library.playlists.firstOrNull { lib.forYou && it.id == lib.playlist }
    // "add" comes right after the player, then the tabs.
    val tabsIndex = nowIndex + 2
    // Ask for recommendations when the tab comes into view, and when the first
    // song to go by turns up meanwhile. Fresh ones are kept.
    val forYouShown = lib.forYou && !s.search.active && opened == null
    val source = s.library.hasHistory || room.queue.isNotEmpty() || s.library.playlists.any { it.tracks.isNotEmpty() }
    LaunchedEffect(forYouShown, source) { if (forYouShown) engine.showRecs() }
    LaunchedEffect(opened?.id) {
        // Another list starts at its top, not where the one before was scrolled to.
        if (list.firstVisibleItemIndex > tabsIndex) list.scrollToItem(tabsIndex)
    }
    BackHandler(enabled = s.search.active) { engine.clearSearch() }
    BackHandler(enabled = opened != null && !s.search.active) { lib.playlist = null }
    val queued = remember(room.queue) { room.queue.mapTo(HashSet()) { it.youtubeId } }
    val saved = remember(s.library.playlists) { s.library.playlists.flatMapTo(HashSet()) { p -> p.tracks.map { it.youtubeId } } }
    val columns = (LocalConfiguration.current.screenWidthDp / 190).coerceAtLeast(2)

    val haptics = LocalHapticFeedback.current
    val drag = remember(list, scope) { QueueDrag(list, scope) }
    drag.onMove = { id, to -> engine.move(id, to) }
    val queueIds = remember(room.queue) { room.queue.map { it.id } }
    // Keep the local order until the server's queue matches it (or give up).
    LaunchedEffect(queueIds, drag.id, drag.order) {
        val o = drag.order
        if (drag.id == null && o != null) {
            if (o != queueIds) delay(2500)
            drag.order = null
        }
    }
    LaunchedEffect(drag.active) { if (drag.active) drag.autoScroll() }
    val display = remember(room.queue, drag.order) {
        val o = drag.order ?: return@remember room.queue
        val byId = room.queue.associateBy { it.id }
        val inOrder = o.toHashSet()
        o.mapNotNull { byId[it] } + room.queue.filter { it.id !in inOrder }
    }
    drag.shown = if (s.search.active || lib.forYou) emptyList() else display.map { it.id }

    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize().queueDrag(drag, haptics),
        state = list,
        contentPadding = PaddingValues(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 96.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item(key = "top") {
            TopBar(engine, s, room, onMembers = { showMembers = true }, onClear = { confirmClear = true }, onSettings = { showSettings = true })
        }
        if (s.conn !is ConnState.Connected) {
            item(key = "conn") { Banner(connText(s.conn), Pal.WARN) }
        }
        if (s.focusLost) {
            item(key = "focus") { Banner(S.FOCUS_LOST, Pal.WARN) { engine.onAppVisible() } }
        }
        item(key = "now") { NowPlaying(engine, s, room, saved, lib) }
        item(key = "add") { AddField(engine, s) }
        if (s.search.active) {
            searchItems(engine, s.search, queued, saved, lib)
        } else {
            item(key = "tabs") { RoomTabs(lib) }
            when {
                opened != null -> playlistItems(engine, opened, lib, queued)
                lib.forYou -> forYouItems(engine, s.library, lib, queued, saved, columns)
                else -> queueItems(engine, room, display, drag, lib)
            }
        }
    }
        AnimatedVisibility(
            visible = showMini && room.currentTrack() != null,
            enter = fadeIn() + slideInVertically { it },
            exit = fadeOut() + slideOutVertically { it },
            modifier = Modifier.align(Alignment.BottomCenter),
        ) {
            MiniPlayer(engine, s, room) { scope.launch { list.animateScrollToItem(0) } }
        }
    }

    LibraryDialogs(engine, s.library, lib)
    if (showMembers) MembersDialog(s) { showMembers = false }
    if (showSettings) SettingsDialog(engine, s) { showSettings = false }
    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text(S.CLEAR_TITLE) },
            text = { Text(S.clearExplain(room.queue.size)) },
            confirmButton = {
                TextButton(onClick = { confirmClear = false; engine.clearQueue() }) {
                    Text(S.CLEAR_CONFIRM, color = Pal.BAD)
                }
            },
            dismissButton = { TextButton(onClick = { confirmClear = false }) { Text(S.CANCEL) } },
            containerColor = Pal.CARD,
        )
    }
}

@Composable
private fun TopBar(
    engine: Engine,
    s: UiState,
    room: RoomState,
    onMembers: () -> Unit,
    onClear: () -> Unit,
    onSettings: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(room.name, fontSize = 22.sp, fontWeight = FontWeight.Bold, color = Pal.TEXT, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                val ok = s.conn is ConnState.Connected
                Dot(if (ok) Pal.GOOD else Pal.WARN)
                Spacer(Modifier.width(6.dp))
                val text = if (ok) {
                    listOfNotNull(S.CONNECTED, s.rttMs?.let { S.rtt(it) }, if (!s.clockSynced) S.CLOCK_SYNCING else null).joinToString(" · ")
                } else {
                    connText(s.conn)
                }
                Text(text, color = Pal.TEXT_DIM, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        Row(
            Modifier
                .clip(RoundedCornerShape(50))
                .background(Pal.CARD)
                .clickable(onClick = onMembers)
                .padding(horizontal = 12.dp, vertical = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Rounded.Group, S.MEMBERS, tint = Pal.TEXT_DIM, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
            Text("${s.members.size}", color = Pal.TEXT, fontSize = 14.sp)
        }
        Box {
            IconButton(onClick = { menu = true }) { Icon(Icons.Rounded.MoreVert, S.MORE, tint = Pal.TEXT_DIM) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = Pal.CARD) {
                MenuItem(if (s.shuffleLeft != null) "${S.SHUFFLE} (${s.shuffleLeft})" else S.SHUFFLE, Icons.Rounded.Shuffle) {
                    menu = false; engine.shuffleUpcoming()
                }
                MenuItem(if (s.clearLeft != null) "${S.CLEAR_QUEUE} (${s.clearLeft})" else S.CLEAR_QUEUE, Icons.Rounded.DeleteSweep, Pal.BAD) {
                    menu = false
                    if (s.clearLeft != null) engine.clearQueue() else onClear()
                }
                MenuItem(S.SETTINGS, Icons.Rounded.Tune) { menu = false; onSettings() }
                HorizontalDivider(color = Pal.BORDER)
                MenuItem(S.LEAVE, Icons.AutoMirrored.Rounded.Logout) { menu = false; engine.leave() }
            }
        }
    }
}

@Composable
internal fun MenuItem(text: String, icon: ImageVector, tint: Color = Pal.TEXT, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(text, color = tint) },
        leadingIcon = { Icon(icon, null, tint = tint) },
        onClick = onClick,
    )
}

@Composable
private fun Banner(text: String, color: Color, onClick: (() -> Unit)? = null) {
    Text(
        text,
        color = color,
        fontSize = 13.sp,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(color.copy(alpha = 0.10f))
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 12.dp, vertical = 9.dp),
    )
}

@Composable
private fun Dot(color: Color, size: Int = 8) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(color))
}

private fun statusOf(s: UiState): Pair<String, Color> {
    val room = s.room
    return when (val p = s.phase) {
        Phase.Idle -> if (room?.playback?.status == PlayStatus.Stopped || room?.currentTrack() == null) {
            S.PHASE_STOPPED to Pal.TEXT_FAINT
        } else {
            S.IDLE to Pal.TEXT_DIM
        }
        Phase.Resolving -> S.PHASE_RESOLVING to Pal.ACCENT
        Phase.Buffering -> S.PHASE_BUFFERING to Pal.ACCENT
        Phase.Waiting -> if (room?.waitingForReady == true) S.PHASE_WAITING to Pal.WARN else S.PHASE_STARTING to Pal.ACCENT
        Phase.Paused -> S.PHASE_PAUSED to Pal.TEXT_DIM
        Phase.Playing -> {
            val d = s.driftMs
            when {
                d != null && abs(d) > Sync.DRIFT_IGNORE_MS -> S.SYNCING to Pal.WARN
                d != null && s.lowLatency -> S.syncedMs(d) to Pal.GOOD
                else -> S.SYNCED to Pal.GOOD
            }
        }
        is Phase.Error -> p.message to Pal.BAD
    }
}

@Composable
private fun NowPlaying(engine: Engine, s: UiState, room: RoomState, saved: Set<String>, lib: LibraryUiState) {
    val track = room.currentTrack()
    Column(
        Modifier
            .fillMaxWidth()
            .clip(CardShape)
            .background(Pal.PANEL)
            .border(1.dp, Pal.BORDER, CardShape)
            .padding(14.dp),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(2f)
                .clip(RoundedCornerShape(10.dp))
                .background(Pal.CARD),
            contentAlignment = Alignment.Center,
        ) {
            if (track != null) {
                AsyncImage(
                    model = YouTubeUrl.thumbUrl(track.youtubeId),
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.fillMaxSize(),
                )
            } else {
                Icon(Icons.Rounded.MusicNote, null, tint = Pal.TEXT_FAINT, modifier = Modifier.size(48.dp))
            }
        }
        Spacer(Modifier.height(12.dp))
        if (track == null) {
            Text(S.NOTHING_PLAYING_TITLE, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = Pal.TEXT)
            Text(S.NOTHING_PLAYING_HINT, fontSize = 14.sp, color = Pal.TEXT_DIM)
        } else {
            Text(track.title, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = Pal.TEXT, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (track.addedBy.isNotEmpty()) {
                    Text("${S.ADDED_BY} ${track.addedBy}", fontSize = 13.sp, color = Pal.TEXT_DIM, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) { StatusPill(s) }
                room.radio?.let { on -> RadioButton(on) { engine.setRadio(!on) } }
                SaveButton(track.youtubeId in saved) { lib.pick(track.toSong()) }
            }
        }
        Spacer(Modifier.height(4.dp))
        SeekBar(engine, track)
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = { engine.previous() }, enabled = track != null, modifier = Modifier.size(56.dp)) {
                Icon(Icons.Rounded.SkipPrevious, null, tint = if (track != null) Pal.TEXT else Pal.TEXT_FAINT, modifier = Modifier.size(34.dp))
            }
            val playing = s.isPlaying || room.waitingForReady
            FilledIconButton(
                onClick = { engine.togglePlay() },
                enabled = track != null,
                modifier = Modifier.size(68.dp),
                colors = IconButtonDefaults.filledIconButtonColors(containerColor = Pal.ACCENT, contentColor = Pal.ON_ACCENT),
            ) {
                Icon(if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow, null, modifier = Modifier.size(38.dp))
            }
            IconButton(onClick = { engine.next() }, enabled = track != null, modifier = Modifier.size(56.dp)) {
                Icon(Icons.Rounded.SkipNext, null, tint = if (track != null) Pal.TEXT else Pal.TEXT_FAINT, modifier = Modifier.size(34.dp))
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                if (s.volume == 0) Icons.Rounded.VolumeOff else Icons.AutoMirrored.Rounded.VolumeUp,
                S.VOLUME, tint = Pal.TEXT_DIM, modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.width(8.dp))
            ThinSlider(
                value = s.volume.toFloat(),
                onValueChange = { engine.setVolume(it.toInt()) },
                valueRange = 0f..100f,
                colors = sliderColors(),
                modifier = Modifier.weight(1f).height(32.dp),
            )
        }
    }
}

/** The room's radio switch: lit in the accent while on. */
@Composable
private fun RadioButton(on: Boolean, onClick: () -> Unit) {
    IconButton(onClick = onClick) {
        Box(
            Modifier
                .size(36.dp)
                .clip(RoundedCornerShape(8.dp))
                .background(if (on) Pal.ACCENT.copy(alpha = 0.16f) else Color.Transparent),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                Icons.Rounded.Radio,
                if (on) S.RADIO_ON else S.RADIO_OFF,
                tint = if (on) Pal.ACCENT else Pal.TEXT_DIM,
                modifier = Modifier.size(22.dp),
            )
        }
    }
}

@Composable
private fun StatusPill(s: UiState) {
    val (st, col) = statusOf(s)
    Row(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(col.copy(alpha = 0.12f))
            .padding(horizontal = 10.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Dot(col, 7)
        Spacer(Modifier.width(6.dp))
        Text(st, fontSize = 12.5.sp, color = col, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Small player pinned to the bottom while the big one is scrolled away. */
@Composable
private fun MiniPlayer(engine: Engine, s: UiState, room: RoomState, onOpen: () -> Unit) {
    val track = room.currentTrack() ?: return
    var pos by remember { mutableLongStateOf(0L) }
    LaunchedEffect(track.id) {
        while (true) {
            pos = engine.positionMs() ?: 0L
            delay(500)
        }
    }
    val dur = track.durationMs ?: 0L
    val frac by animateFloatAsState(if (dur > 0) (pos.toFloat() / dur).coerceIn(0f, 1f) else 0f, label = "mini")
    val shape = RoundedCornerShape(16.dp)
    Column(
        Modifier
            .navigationBarsPadding()
            .padding(horizontal = 10.dp, vertical = 8.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(Pal.RAISED)
            .border(1.dp, Pal.BORDER, shape)
            .clickable(onClick = onOpen),
    ) {
        Row(Modifier.padding(start = 8.dp, top = 8.dp, bottom = 8.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Thumb(YouTubeUrl.thumbUrl(track.youtubeId), Modifier.width(64.dp))
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(track.title, color = Pal.TEXT, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                val (st, col) = statusOf(s)
                Text(st, color = col, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            val playing = s.isPlaying || room.waitingForReady
            IconButton(onClick = { engine.togglePlay() }) {
                Icon(if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow, null, tint = Pal.TEXT, modifier = Modifier.size(30.dp))
            }
            IconButton(onClick = { engine.next() }) {
                Icon(Icons.Rounded.SkipNext, null, tint = Pal.TEXT_DIM, modifier = Modifier.size(28.dp))
            }
        }
        Box(Modifier.fillMaxWidth().height(3.dp).background(Pal.BORDER)) {
            Box(Modifier.fillMaxWidth(frac).height(3.dp).background(Pal.ACCENT))
        }
    }
}

/** Slim slider (thin track, round thumb) that fits a phone better. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ThinSlider(
    value: Float,
    onValueChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    valueRange: ClosedFloatingPointRange<Float> = 0f..1f,
    onValueChangeFinished: (() -> Unit)? = null,
    @Suppress("UNUSED_PARAMETER") colors: SliderColors? = null,
) {
    val span = (valueRange.endInclusive - valueRange.start).takeIf { it > 0f } ?: 1f
    Slider(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier.height(32.dp),
        enabled = enabled,
        valueRange = valueRange,
        onValueChangeFinished = onValueChangeFinished,
        thumb = {
            Box(
                Modifier
                    .size(16.dp)
                    .clip(CircleShape)
                    .background(if (enabled) Pal.ACCENT else Pal.TEXT_FAINT),
            )
        },
        track = { st ->
            val f = ((st.value - valueRange.start) / span).coerceIn(0f, 1f)
            Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(50)).background(Pal.BORDER)) {
                Box(Modifier.fillMaxWidth(f).height(4.dp).background(if (enabled) Pal.ACCENT else Pal.TEXT_FAINT))
            }
        },
    )
}

@Composable
private fun sliderColors() = SliderDefaults.colors(
    thumbColor = Pal.ACCENT,
    activeTrackColor = Pal.ACCENT,
    inactiveTrackColor = Pal.BORDER,
)

@Composable
private fun SeekBar(engine: Engine, track: Track?) {
    var pos by remember { mutableLongStateOf(0L) }
    var dragging by remember { mutableStateOf(false) }
    var dragValue by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(track?.id) {
        while (true) {
            pos = engine.positionMs() ?: 0L
            delay(250)
        }
    }
    val dur = track?.durationMs ?: 0L
    val shown = if (dragging) dragValue.toLong() else pos
    Column {
        ThinSlider(
            value = if (dur > 0) shown.coerceIn(0, dur).toFloat() else 0f,
            onValueChange = { dragging = true; dragValue = it },
            onValueChangeFinished = {
                if (dragging) engine.seek(dragValue.toLong())
                dragging = false
            },
            valueRange = 0f..(if (dur > 0) dur.toFloat() else 1f),
            enabled = track != null && dur > 0,
            colors = sliderColors(),
            modifier = Modifier.height(32.dp),
        )
        Row(Modifier.fillMaxWidth()) {
            Text(if (track != null) fmt(shown) else "", color = Pal.TEXT_DIM, fontSize = 12.sp)
            Spacer(Modifier.weight(1f))
            Text(if (dur > 0) fmt(dur) else "", color = Pal.TEXT_DIM, fontSize = 12.sp)
        }
    }
}

fun fmt(ms: Long): String {
    val t = (ms.coerceAtLeast(0) / 1000)
    val h = t / 3600
    val m = (t % 3600) / 60
    val sec = t % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}

@Composable
private fun AddField(engine: Engine, s: UiState) {
    var text by rememberSaveable { mutableStateOf("") }
    val clipboard = LocalClipboardManager.current
    val focus = LocalFocusManager.current
    fun go(t: String) {
        if (t.isBlank()) return
        engine.submit(t)
        if (YouTubeUrl.parse(t.trim()) != null) text = ""
        focus.clearFocus()
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        OutlinedTextField(
            value = text,
            onValueChange = { text = it },
            placeholder = { Text(S.PASTE_LINK, color = Pal.TEXT_FAINT, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            leadingIcon = { Icon(Icons.Rounded.Search, null, tint = Pal.TEXT_DIM) },
            trailingIcon = {
                if (text.isEmpty()) {
                    IconButton(onClick = {
                        val clip = clipboard.getText()?.text?.trim().orEmpty()
                        if (clip.isNotEmpty()) {
                            if (YouTubeUrl.parse(clip) != null) go(clip) else text = clip
                        }
                    }) { Icon(Icons.Rounded.ContentPaste, S.PASTE, tint = Pal.TEXT_DIM) }
                } else {
                    Row {
                        IconButton(onClick = { text = "" }) { Icon(Icons.Rounded.Close, S.CLEAR_TEXT, tint = Pal.TEXT_DIM) }
                        IconButton(onClick = { go(text) }) { Icon(Icons.AutoMirrored.Rounded.Send, S.SEND, tint = Pal.ACCENT) }
                    }
                }
            },
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search, keyboardType = KeyboardType.Uri),
            keyboardActions = KeyboardActions(onSearch = { go(text) }),
            shape = RoundedCornerShape(12.dp),
            colors = fieldColors(),
            modifier = Modifier.fillMaxWidth(),
        )
        if (s.pendingAdds.isNotEmpty()) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 4.dp)) {
                CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = Pal.ACCENT)
                Spacer(Modifier.width(8.dp))
                Text(
                    if (s.pendingAdds.size > 1) "${S.ADDING} (${s.pendingAdds.size})" else S.ADDING,
                    color = Pal.TEXT_DIM, fontSize = 13.sp,
                )
            }
        }
        if (s.library.importing > 0) Working(S.IMPORTING, Modifier.padding(start = 4.dp))
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.searchItems(
    engine: Engine,
    sv: SearchView,
    queued: Set<String>,
    saved: Set<String>,
    lib: LibraryUiState,
) {
    item(key = "search-head") {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            IconButton(onClick = { engine.clearSearch() }) {
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, S.BACK, tint = Pal.TEXT_DIM)
            }
            Chip(S.SEARCH_SONGS, !sv.playlists) { if (sv.playlists) engine.search(sv.query, false) }
            Chip(S.SEARCH_PLAYLISTS, sv.playlists) { if (!sv.playlists) engine.search(sv.query, true) }
        }
    }
    when {
        sv.loading -> item(key = "search-loading") {
            Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = Pal.ACCENT)
                Spacer(Modifier.width(10.dp))
                Text(S.SEARCHING, color = Pal.TEXT_DIM)
            }
        }
        sv.error != null -> item(key = "search-error") {
            Text(sv.error, color = Pal.TEXT_DIM, modifier = Modifier.padding(12.dp))
        }
        else -> items(sv.results, key = { "hit-" + (it.youtubeId ?: it.playlistId) }) { h ->
            SearchRow(h, h.youtubeId in queued, h.youtubeId in saved, onSave = { lib.pick(h) }) { engine.addHit(h) }
        }
    }
}

@Composable
internal fun Chip(text: String, selected: Boolean, onClick: () -> Unit) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = { Text(text) },
        colors = FilterChipDefaults.filterChipColors(
            selectedContainerColor = Pal.ACCENT.copy(alpha = 0.2f),
            selectedLabelColor = Pal.TEXT,
            labelColor = Pal.TEXT_DIM,
        ),
    )
}

@Composable
internal fun SearchRow(h: SearchHit, queued: Boolean, saved: Boolean, onSave: () -> Unit, onAdd: () -> Unit) {
    var tapped by remember { mutableStateOf(false) }
    fun add() {
        tapped = true
        onAdd()
    }
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(Pal.PANEL)
            .clickable(onClickLabel = S.ADD) { add() }
            .padding(start = 8.dp, top = 8.dp, bottom = 8.dp, end = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Thumb(h.thumb, Modifier.width(88.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(h.title, color = Pal.TEXT, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            val meta = listOfNotNull(
                h.uploader.takeIf { it.isNotEmpty() },
                h.durationMs?.let { fmt(it) },
                h.count?.let { "${S.PLAYLIST_TAG} · ${S.songs(it.toInt())}" } ?: if (h.playlistId != null) S.PLAYLIST_TAG else null,
            ).joinToString(" · ")
            Text(meta, color = Pal.TEXT_DIM, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        SaveButton(saved, Modifier.size(42.dp), onSave)
        QueueButton(queued || tapped, Modifier.size(42.dp)) { add() }
    }
}

@Composable
internal fun Thumb(url: String?, modifier: Modifier) {
    Box(
        modifier
            .aspectRatio(16f / 9f)
            .clip(RoundedCornerShape(6.dp))
            .background(Pal.CARD),
        contentAlignment = Alignment.Center,
    ) {
        if (url != null) {
            AsyncImage(model = url, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            Icon(Icons.Rounded.MusicNote, null, tint = Pal.TEXT_FAINT, modifier = Modifier.size(18.dp))
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.queueItems(
    engine: Engine,
    room: RoomState,
    display: List<Track>,
    drag: QueueDrag,
    lib: LibraryUiState,
) {
    item(key = "queue-head") {
        Column(Modifier.padding(start = 4.dp, bottom = 2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.AutoMirrored.Rounded.QueueMusic, null, tint = Pal.TEXT_DIM, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text(S.songs(room.queue.size), color = Pal.TEXT_DIM, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }
            if (room.queue.size > 1) {
                Text(S.QUEUE_DRAG_HINT, color = Pal.TEXT_FAINT, fontSize = 12.sp, modifier = Modifier.padding(start = 26.dp))
            }
        }
    }
    if (room.queue.isEmpty()) {
        item(key = "queue-empty") {
            Column(Modifier.fillMaxWidth().padding(vertical = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text(S.QUEUE_EMPTY_TITLE, color = Pal.TEXT, fontSize = 16.sp, fontWeight = FontWeight.Medium)
                Text(S.QUEUE_EMPTY_HINT, color = Pal.TEXT_DIM, fontSize = 13.sp)
            }
        }
        return
    }
    val currentId = room.playback.trackIndex?.let { room.queue.getOrNull(it) }?.id
    val currentPos = display.indexOfFirst { it.id == currentId }
    itemsIndexed(display, key = { _, t -> t.id }) { i, t ->
        val dragging = drag.id == t.id
        val shape = RoundedCornerShape(10.dp)
        QueueRow(
            engine,
            drag,
            t,
            i,
            current = i == currentPos,
            size = display.size,
            past = currentPos >= 0 && i < currentPos,
            lifted = dragging,
            onSave = { lib.pick(t.toSong()) },
            modifier = Modifier
                .zIndex(if (dragging) 1f else 0f)
                .animateItem(
                    placementSpec = if (dragging) null else spring(Spring.DampingRatioNoBouncy, Spring.StiffnessMediumLow, IntOffset.VisibilityThreshold),
                )
                .graphicsLayer {
                    translationY = drag.translation(t.id)
                    if (dragging) {
                        shadowElevation = 10.dp.toPx()
                        this.shape = shape
                        scaleX = 1.03f
                        scaleY = 1.03f
                    }
                },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun QueueRow(
    engine: Engine,
    drag: QueueDrag,
    t: Track,
    index: Int,
    current: Boolean,
    size: Int,
    past: Boolean,
    lifted: Boolean,
    onSave: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val menu = drag.menu == t.id
    val close = { drag.menu = null }
    val shape = RoundedCornerShape(10.dp)
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(
                    when {
                        lifted -> Pal.RAISED
                        current -> Pal.ACCENT.copy(alpha = 0.12f)
                        else -> Pal.PANEL
                    },
                )
                .then(if (current || lifted) Modifier.border(1.dp, Pal.ACCENT.copy(alpha = 0.5f), shape) else Modifier)
                .clickable { engine.playTrack(t.id) }
                .padding(start = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(26.dp)) {
                if (current) {
                    Icon(Icons.Rounded.GraphicEq, null, tint = Pal.ACCENT, modifier = Modifier.size(18.dp))
                } else {
                    Text("${index + 1}", color = Pal.TEXT_FAINT, fontSize = 12.sp)
                }
            }
            Thumb(YouTubeUrl.thumbUrl(t.youtubeId), Modifier.width(72.dp))
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    t.title,
                    color = if (past) Pal.TEXT_DIM else Pal.TEXT,
                    fontSize = 14.sp,
                    fontWeight = if (current) FontWeight.SemiBold else FontWeight.Normal,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                val meta = listOfNotNull(t.addedBy.takeIf { it.isNotEmpty() }, t.durationMs?.let { fmt(it) }).joinToString(" · ")
                if (meta.isNotEmpty()) Text(meta, color = Pal.TEXT_FAINT, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            IconButton(onClick = { drag.menu = t.id }) { Icon(Icons.Rounded.MoreVert, S.MORE, tint = Pal.TEXT_FAINT) }
        }
        DropdownMenu(expanded = menu, onDismissRequest = close, containerColor = Pal.CARD) {
            MenuItem(S.PLAY_THIS, Icons.Rounded.PlayArrow) { close(); engine.playTrack(t.id) }
            if (!current) MenuItem(S.PLAY_NEXT, Icons.AutoMirrored.Rounded.PlaylistPlay) { close(); engine.playNext(t.id) }
            if (index > 0) MenuItem(S.MOVE_UP, Icons.Rounded.KeyboardArrowUp) { close(); engine.move(t.id, index - 1) }
            if (index < size - 1) MenuItem(S.MOVE_DOWN, Icons.Rounded.KeyboardArrowDown) { close(); engine.move(t.id, index + 1) }
            MenuItem(S.SAVE_TO_PLAYLIST, Icons.AutoMirrored.Rounded.PlaylistAdd) { close(); onSave() }
            MenuItem(S.REMOVE, Icons.Rounded.Delete, Pal.BAD) { close(); engine.remove(t.id) }
        }
    }
}

// ----- members ----------------------------------------------------------------------

private fun memberDot(m: Member, playing: Boolean): Pair<Color, String?> {
    val s = m.sync ?: return Pal.TEXT_FAINT to null
    if (s.loading) return Pal.WARN to S.DOT_LOADING
    val d = s.driftMs
    return when {
        d != null && abs(d) > Sync.DRIFT_IGNORE_MS -> (if (abs(d) > 250) Pal.BAD else Pal.WARN) to S.dotDrift(d)
        d != null -> Pal.GOOD to S.DOT_SYNCED
        playing -> Pal.TEXT_FAINT to S.DOT_WAITING
        else -> Pal.TEXT_FAINT to null
    }
}

@Composable
internal fun SettingsDialog(engine: Engine, s: UiState, onClose: () -> Unit) {
    var offset by remember { mutableStateOf(s.audioOffsetMs.toFloat()) }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(S.SETTINGS) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(S.NORMALIZE, color = Pal.TEXT, fontSize = 16.sp, modifier = Modifier.weight(1f))
                    Switch(
                        checked = s.normalize,
                        onCheckedChange = { engine.setNormalize(it) },
                        colors = SwitchDefaults.colors(checkedTrackColor = Pal.ACCENT),
                    )
                }
                Text(S.NORMALIZE_EXPLAIN, color = Pal.TEXT_DIM, fontSize = 13.sp)
                HorizontalDivider(color = Pal.BORDER, modifier = Modifier.padding(vertical = 8.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(S.LOW_LATENCY_TITLE, color = Pal.TEXT, fontSize = 16.sp, modifier = Modifier.weight(1f))
                    Switch(
                        checked = s.lowLatency,
                        onCheckedChange = { engine.setLowLatency(it) },
                        colors = SwitchDefaults.colors(checkedTrackColor = Pal.ACCENT),
                    )
                }
                Text(S.LOW_LATENCY_EXPLAIN, color = Pal.TEXT_DIM, fontSize = 13.sp)
                HorizontalDivider(color = Pal.BORDER, modifier = Modifier.padding(vertical = 8.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(S.AUDIO_OFFSET_TITLE, color = Pal.TEXT, fontSize = 16.sp, modifier = Modifier.weight(1f))
                    Text(S.offset(offset.toInt()), color = Pal.TEXT, fontSize = 14.sp)
                }
                ThinSlider(
                    value = offset,
                    onValueChange = { offset = (Math.round(it / 5f) * 5).toFloat() },
                    onValueChangeFinished = { engine.setAudioOffset(offset.toInt()) },
                    valueRange = -300f..300f,
                    colors = sliderColors(),
                )
                Text(S.AUDIO_OFFSET_EXPLAIN, color = Pal.TEXT_DIM, fontSize = 13.sp)
            }
        },
        confirmButton = { TextButton(onClick = onClose) { Text("OK") } },
        containerColor = Pal.CARD,
    )
}

@Composable
private fun MembersDialog(s: UiState, onClose: () -> Unit) {
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(S.membersTitle(s.members.size)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                s.members.forEach { m ->
                    val (color, label) = memberDot(m, s.isPlaying)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Dot(color, 10)
                        Spacer(Modifier.width(10.dp))
                        Text(m.name, color = Pal.TEXT, fontSize = 15.sp)
                        if (m.id == s.me?.id) Text(" ${S.YOU}", color = Pal.TEXT_FAINT, fontSize = 15.sp)
                        Spacer(Modifier.weight(1f))
                        if (label != null) Text(label, color = Pal.TEXT_DIM, fontSize = 13.sp)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onClose) { Text("OK") } },
        containerColor = Pal.CARD,
    )
}

// ----- toasts -------------------------------------------------------------------------

@Composable
private fun Toasts(engine: Engine, toasts: List<ToastMsg>, modifier: Modifier) {
    Column(
        modifier.padding(horizontal = 16.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        toasts.takeLast(3).forEach { t ->
            androidx.compose.runtime.key(t.id) {
                var visible by remember { mutableStateOf(false) }
                LaunchedEffect(Unit) {
                    visible = true
                    delay(if (t.error) 5000 else 3500)
                    visible = false
                    delay(250)
                    engine.dismissToast(t.id)
                }
                AnimatedVisibility(
                    visible = visible,
                    enter = fadeIn() + slideInVertically { it / 2 },
                    exit = fadeOut() + slideOutVertically { it / 2 },
                ) {
                    Surface(
                        color = Pal.CARD,
                        shape = RoundedCornerShape(12.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .border(1.dp, if (t.error) Pal.BAD.copy(alpha = 0.6f) else Pal.BORDER, RoundedCornerShape(12.dp))
                            .clickable { engine.dismissToast(t.id) },
                    ) {
                        Text(
                            t.text,
                            color = if (t.error) Pal.BAD else Pal.TEXT,
                            fontSize = 14.sp,
                            modifier = Modifier.padding(horizontal = 14.dp, vertical = 11.dp),
                        )
                    }
                }
            }
        }
    }
}
