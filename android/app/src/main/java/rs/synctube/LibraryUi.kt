// The "Za tebe" tab (own playlists and recommendations) and its dialogs.
package rs.synctube

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.PlaylistAdd
import androidx.compose.material.icons.automirrored.rounded.PlaylistAddCheck
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.MoreVert
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

internal sealed class LibraryDialog {
    /** Choosing the playlist to save something to. */
    data class Pick(val item: SaveItem, val title: String) : LibraryDialog()
    data object Create : LibraryDialog()
    data class Rename(val id: String) : LibraryDialog()
    data class Delete(val id: String) : LibraryDialog()
    data object ClearHistory : LibraryDialog()
}

/** What the tab and its dialogs are showing. */
@Stable
internal class LibraryUiState(forYou: Boolean = false, playlist: String? = null) {
    /** "Za tebe" is selected instead of the queue. */
    var forYou by mutableStateOf(forYou)

    /** The playlist being looked at. */
    var playlist by mutableStateOf(playlist)
    var dialog by mutableStateOf<LibraryDialog?>(null)

    fun pick(h: SearchHit) {
        val song = h.toSong()
        val item = if (song != null) SaveItem.Songs(listOf(song)) else SaveItem.YoutubePlaylist(h.playlistId ?: return)
        dialog = LibraryDialog.Pick(item, h.title)
    }

    fun pick(song: Song) {
        dialog = LibraryDialog.Pick(SaveItem.Songs(listOf(song)), song.title)
    }

    companion object {
        val Saver = listSaver<LibraryUiState, Any?>(
            save = { listOf(it.forYou, it.playlist) },
            restore = { LibraryUiState(it[0] as Boolean, it[1] as String?) },
        )
    }
}

private fun Song.toHit() = SearchHit(youtubeId, null, title, channel.orEmpty(), durationMs, null, YouTubeUrl.thumbUrl(youtubeId))

private val RowShape = RoundedCornerShape(10.dp)

@Composable
internal fun RoomTabs(lib: LibraryUiState) {
    Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip(S.QUEUE, !lib.forYou) { lib.forYou = false }
        Chip(S.TAB_FOR_YOU, lib.forYou) { lib.forYou = true }
    }
}

/** Spinner with a line of text. */
@Composable
internal fun Working(text: String, modifier: Modifier = Modifier) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = Pal.ACCENT)
        Spacer(Modifier.width(8.dp))
        Text(text, color = Pal.TEXT_DIM, fontSize = 13.sp)
    }
}

/** Opens the picker; a check once the song is in one of the playlists. */
@Composable
internal fun SaveButton(saved: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    IconButton(onClick = onClick, modifier = modifier) {
        Icon(
            if (saved) Icons.AutoMirrored.Rounded.PlaylistAddCheck else Icons.AutoMirrored.Rounded.PlaylistAdd,
            S.SAVE_TO_PLAYLIST,
            tint = if (saved) Pal.GOOD else Pal.TEXT_DIM,
            modifier = Modifier.size(24.dp),
        )
    }
}

@Composable
private fun SectionTitle(text: String, modifier: Modifier) {
    Text(
        text, color = Pal.TEXT, fontSize = 16.sp, fontWeight = FontWeight.SemiBold,
        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier,
    )
}

@Composable
private fun Empty(title: String, hint: String) {
    Column(Modifier.fillMaxWidth().padding(vertical = 24.dp, horizontal = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, color = Pal.TEXT, fontSize = 16.sp, fontWeight = FontWeight.Medium)
        Text(hint, color = Pal.TEXT_DIM, fontSize = 13.sp, textAlign = TextAlign.Center)
    }
}

// ----- playlists and recommendations ------------------------------------------------

internal fun LazyListScope.forYouItems(
    engine: Engine,
    lv: LibraryView,
    lib: LibraryUiState,
    queued: Set<String>,
    saved: Set<String>,
    columns: Int,
) {
    item(key = "lists-head") {
        Row(Modifier.padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            SectionTitle(S.MY_PLAYLISTS, Modifier.weight(1f))
            TextButton(onClick = { lib.dialog = LibraryDialog.Create }) {
                Icon(Icons.Rounded.Add, null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(S.NEW_PLAYLIST)
            }
        }
    }
    if (lv.playlists.isEmpty()) {
        item(key = "lists-empty") {
            Text(S.NO_PLAYLISTS, color = Pal.TEXT_DIM, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 4.dp))
        }
    }
    items(lv.playlists.chunked(columns), key = { "lists-" + it.first().id }) { row ->
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            row.forEach { p -> PlaylistCard(p, Modifier.weight(1f)) { lib.playlist = p.id } }
            repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
        }
    }
    item(key = "recs-head") {
        Row(Modifier.padding(start = 4.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            SectionTitle(S.RECOMMENDED, Modifier.weight(1f))
            if (lv.hasHistory) {
                TextButton(onClick = { lib.dialog = LibraryDialog.ClearHistory }) {
                    Text(S.CLEAR_HISTORY, color = Pal.TEXT_DIM, maxLines = 1)
                }
            }
            TextButton(onClick = { engine.refreshRecs() }, enabled = !lv.recsLoading) {
                Icon(Icons.Rounded.Refresh, null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(S.REFRESH, maxLines = 1)
            }
        }
    }
    if (lv.recsLoading) item(key = "recs-loading") { Working(S.RECS_LOADING, Modifier.padding(4.dp)) }
    lv.recsError?.let { e ->
        item(key = "recs-error") { Text(e, color = Pal.WARN, fontSize = 13.sp, modifier = Modifier.padding(4.dp)) }
    }
    if (lv.recs.isEmpty()) {
        if (!lv.recsLoading && lv.recsError == null) item(key = "recs-empty") { Empty(S.RECS_EMPTY, S.RECS_EMPTY_HINT) }
        return
    }
    if (!lv.hasHistory && lv.playlists.all { it.tracks.isEmpty() }) {
        item(key = "recs-why") {
            Text(S.RECS_FROM_QUEUE, color = Pal.TEXT_DIM, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 4.dp))
        }
    }
    items(lv.recs, key = { "rec-" + it.youtubeId }) { song ->
        SearchRow(song.toHit(), song.youtubeId in queued, song.youtubeId in saved, onSave = { lib.pick(song) }) {
            engine.addSong(song.youtubeId)
        }
    }
}

@Composable
private fun PlaylistCard(p: Playlist, modifier: Modifier, onOpen: () -> Unit) {
    Column(
        modifier
            .clip(CardShape)
            .background(Pal.PANEL)
            .border(1.dp, Pal.BORDER, CardShape)
            .clickable(onClick = onOpen)
            .padding(8.dp),
    ) {
        Thumb(p.tracks.firstOrNull()?.let { YouTubeUrl.thumbUrl(it.youtubeId) }, Modifier.fillMaxWidth())
        Spacer(Modifier.height(6.dp))
        Text(p.name, color = Pal.TEXT, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(S.songs(p.tracks.size), color = Pal.TEXT_DIM, fontSize = 12.sp)
    }
}

internal fun LazyListScope.playlistItems(engine: Engine, p: Playlist, lib: LibraryUiState, queued: Set<String>) {
    item(key = "list-head") {
        Column {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { lib.playlist = null }) {
                    Icon(Icons.AutoMirrored.Rounded.ArrowBack, S.BACK, tint = Pal.TEXT_DIM)
                }
                Column(Modifier.weight(1f)) {
                    Text(p.name, color = Pal.TEXT, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    val total = p.tracks.sumOf { it.durationMs ?: 0L }
                    val meta = listOfNotNull(S.songs(p.tracks.size), if (total > 0) fmt(total) else null).joinToString(" · ")
                    Text(meta, color = Pal.TEXT_DIM, fontSize = 12.sp)
                }
                IconButton(onClick = { lib.dialog = LibraryDialog.Rename(p.id) }) {
                    Icon(Icons.Rounded.Edit, S.RENAME, tint = Pal.TEXT_DIM)
                }
                IconButton(onClick = { lib.dialog = LibraryDialog.Delete(p.id) }) {
                    Icon(Icons.Rounded.Delete, S.DELETE_PLAYLIST, tint = Pal.BAD)
                }
            }
            Button(
                onClick = { engine.queuePlaylist(p.id) },
                enabled = p.tracks.isNotEmpty(),
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
            ) {
                Icon(Icons.Rounded.Add, null, modifier = Modifier.size(20.dp))
                Spacer(Modifier.width(8.dp))
                Text(S.ADD_ALL)
            }
        }
    }
    if (p.tracks.isEmpty()) item(key = "list-empty") { Empty(S.PLAYLIST_EMPTY, S.PLAYLIST_EMPTY_HINT) }
    itemsIndexed(p.tracks, key = { _, t -> "saved-" + t.youtubeId }) { i, t ->
        SavedRow(engine, p, i, t, t.youtubeId in queued, Modifier.animateItem())
    }
}

/** A song of one of the user's playlists. */
@Composable
private fun SavedRow(engine: Engine, p: Playlist, index: Int, t: Song, queued: Boolean, modifier: Modifier) {
    var menu by remember { mutableStateOf(false) }
    var tapped by remember { mutableStateOf(false) }
    fun add() {
        tapped = true
        engine.addSong(t.youtubeId)
    }
    Row(
        modifier
            .fillMaxWidth()
            .clip(RowShape)
            .background(Pal.PANEL)
            .padding(start = 8.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("${index + 1}", color = Pal.TEXT_FAINT, fontSize = 12.sp, modifier = Modifier.width(26.dp))
        Thumb(YouTubeUrl.thumbUrl(t.youtubeId), Modifier.width(72.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(t.title, color = Pal.TEXT, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            val meta = listOfNotNull(t.channel, t.durationMs?.let { fmt(it) }).joinToString(" · ")
            if (meta.isNotEmpty()) Text(meta, color = Pal.TEXT_FAINT, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        QueueButton(queued || tapped, Modifier.size(44.dp)) { add() }
        Box {
            IconButton(onClick = { menu = true }, modifier = Modifier.size(44.dp)) {
                Icon(Icons.Rounded.MoreVert, S.MORE, tint = Pal.TEXT_FAINT)
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = Pal.CARD) {
                MenuItem(S.ADD, Icons.Rounded.Add) { menu = false; add() }
                if (index > 0) {
                    MenuItem(S.MOVE_UP, Icons.Rounded.KeyboardArrowUp) { menu = false; engine.moveSaved(p.id, t.youtubeId, index - 1) }
                }
                if (index < p.tracks.size - 1) {
                    MenuItem(S.MOVE_DOWN, Icons.Rounded.KeyboardArrowDown) { menu = false; engine.moveSaved(p.id, t.youtubeId, index + 1) }
                }
                MenuItem(S.REMOVE_FROM_PLAYLIST, Icons.Rounded.Delete, Pal.BAD) { menu = false; engine.unsave(p.id, t.youtubeId) }
            }
        }
    }
}

/** Plus; a check once the song was added. */
@Composable
internal fun QueueButton(added: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    IconButton(onClick = onClick, modifier = modifier) {
        Icon(
            if (added) Icons.Rounded.Check else Icons.Rounded.Add,
            S.ADD,
            tint = if (added) Pal.GOOD else Pal.ACCENT,
            modifier = Modifier.size(24.dp),
        )
    }
}

// ----- dialogs ------------------------------------------------------------------------

@Composable
internal fun LibraryDialogs(engine: Engine, lv: LibraryView, lib: LibraryUiState) {
    val close = { lib.dialog = null }
    when (val d = lib.dialog) {
        null -> {}
        is LibraryDialog.Pick -> PickDialog(engine, lv, d, close)
        LibraryDialog.Create -> NameDialog(S.NEW_PLAYLIST, S.CREATE, "", close) { engine.createPlaylist(it) }
        is LibraryDialog.Rename -> {
            val p = lv.playlists.firstOrNull { it.id == d.id } ?: return
            NameDialog(S.RENAME, S.SAVE, p.name, close) { engine.renamePlaylist(p.id, it) }
        }
        is LibraryDialog.Delete -> {
            val p = lv.playlists.firstOrNull { it.id == d.id } ?: return
            ConfirmDialog(S.deletePlaylistTitle(p.name), S.deletePlaylistExplain(p.tracks.size), close) {
                lib.playlist = null
                engine.deletePlaylist(p.id)
            }
        }
        LibraryDialog.ClearHistory -> ConfirmDialog(S.CLEAR_HISTORY_TITLE, S.CLEAR_HISTORY_EXPLAIN, close) { engine.clearHistory() }
    }
}

@Composable
private fun ConfirmDialog(title: String, text: String, close: () -> Unit, onYes: () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        title = { Text(title) },
        text = { Text(text) },
        confirmButton = { TextButton(onClick = { close(); onYes() }) { Text(S.DELETE, color = Pal.BAD) } },
        dismissButton = { TextButton(onClick = close) { Text(S.CANCEL) } },
        containerColor = Pal.CARD,
    )
}

@Composable
private fun NameField(value: String, onChange: (String) -> Unit, label: String, modifier: Modifier, onDone: () -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = { if (it.codePointCount(0, it.length) <= Library.MAX_NAME_CHARS) onChange(it) },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        shape = RowShape,
        colors = fieldColors(),
        modifier = modifier,
    )
}

@Composable
private fun NameDialog(title: String, button: String, start: String, close: () -> Unit, onName: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf(start) }
    val focus = remember { FocusRequester() }
    fun done() {
        if (name.isBlank()) return
        close()
        onName(name)
    }
    AlertDialog(
        onDismissRequest = close,
        title = { Text(title) },
        text = {
            NameField(name, { name = it }, S.PLAYLIST_NAME, Modifier.fillMaxWidth().focusRequester(focus)) { done() }
            // In here, because the dialog's content is composed after the dialog itself.
            LaunchedEffect(Unit) { focus.requestFocus() }
        },
        confirmButton = { TextButton(onClick = { done() }, enabled = name.isNotBlank()) { Text(button) } },
        dismissButton = { TextButton(onClick = close) { Text(S.CANCEL) } },
        containerColor = Pal.CARD,
    )
}

@Composable
private fun PickDialog(engine: Engine, lv: LibraryView, d: LibraryDialog.Pick, close: () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        title = { Text(if (d.item is SaveItem.YoutubePlaylist) S.SAVE_YT_PLAYLIST else S.SAVE_TO_PLAYLIST, fontSize = 20.sp) },
        text = { PickContent(lv, d) { close(); engine.save(it, d.item) } },
        confirmButton = { TextButton(onClick = close) { Text(S.CLOSE) } },
        containerColor = Pal.CARD,
    )
}

@Composable
internal fun PickContent(lv: LibraryView, d: LibraryDialog.Pick, onPick: (SaveTo) -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    fun create() {
        if (name.isNotBlank()) onPick(SaveTo.New(name))
    }
    Column {
        Text(d.title, color = Pal.TEXT_DIM, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(10.dp))
        LazyColumn(Modifier.weight(1f, fill = false).heightIn(max = 300.dp)) {
            items(lv.playlists, key = { it.id }) { p ->
                val has = d.item is SaveItem.Songs && d.item.songs.all { p.contains(it.youtubeId) }
                PickRow(p, has) { onPick(SaveTo.Existing(p.id)) }
            }
        }
        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            NameField(name, { name = it }, S.NEW_PLAYLIST_HINT, Modifier.weight(1f)) { create() }
            TextButton(onClick = { create() }, enabled = name.isNotBlank()) { Text(S.CREATE) }
        }
    }
}

@Composable
private fun PickRow(p: Playlist, has: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RowShape)
            .clickable(enabled = !has, onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(p.name, color = if (has) Pal.TEXT_DIM else Pal.TEXT, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val meta = listOfNotNull(S.songs(p.tracks.size), if (has) S.ALREADY_SAVED else null).joinToString(" · ")
            Text(meta, color = Pal.TEXT_FAINT, fontSize = 12.sp)
        }
        if (has) Icon(Icons.Rounded.Check, S.ALREADY_SAVED, tint = Pal.GOOD, modifier = Modifier.size(20.dp))
    }
}
