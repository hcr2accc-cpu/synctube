// The user's own playlists, what they listened to, and the picking and ranking
// of recommendations. Mirrors client/src/library.rs. Everything stays on this
// phone (playlists.json and history.json); no network here.
package rs.synctube

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.UUID
import kotlin.math.pow
import kotlin.random.Random

@Serializable
data class Song(
    @SerialName("youtube_id") val youtubeId: String,
    val title: String,
    @SerialName("duration_ms") val durationMs: Long? = null,
    val channel: String? = null,
)

fun Track.toSong() = Song(youtubeId, title, durationMs)
fun TrackInfo.toSong() = Song(youtubeId, title, durationMs)

/** A search result as a song to save (null for a playlist). */
fun SearchHit.toSong() = youtubeId?.let { Song(it, title, durationMs, uploader.ifEmpty { null }) }

@Serializable
data class Playlist(val id: String, val name: String, val tracks: List<Song> = emptyList()) {
    fun contains(youtubeId: String) = tracks.any { it.youtubeId == youtubeId }
}

/** What is known about a song the user listened to. */
@Serializable
data class Heard(
    val title: String = "",
    @SerialName("duration_ms") val durationMs: Long? = null,
    val plays: Int = 0,
    @SerialName("own_plays") val ownPlays: Int = 0,
    /** Taste score as of [lastUnix]; halves every 14 days. */
    val score: Double = 0.0,
    @SerialName("last_unix") val lastUnix: Long = 0,
) {
    fun scoreAt(now: Long): Double =
        score * 0.5.pow((now - lastUnix).coerceAtLeast(0).toDouble() / Library.HALF_LIFE_SECS)
}

@Serializable
data class Recs(@SerialName("at_unix") val atUnix: Long = 0, val items: List<Song> = emptyList())

@Serializable
data class History(val songs: Map<String, Heard> = emptyMap(), val recs: Recs = Recs())

/** A song to look for similar ones to, and how much it counts. */
data class Seed(val song: Song, val weight: Double)
data class Mix(val weight: Double, val songs: List<Song>)

/** Outcome of adding songs to a playlist; [noRoom] were left out because it is full. */
data class Added(val added: Int, val noRoom: Int)

/** The playlist something is saved to. */
sealed class SaveTo {
    data class Existing(val id: String) : SaveTo()
    data class New(val name: String) : SaveTo()
}

/** What is saved to one of the user's playlists. */
sealed class SaveItem {
    data class Songs(val songs: List<Song>) : SaveItem()
    /** Every song of a YouTube playlist. */
    data class YoutubePlaylist(val id: String) : SaveItem()
}

/**
 * Playlists and history. The contents are immutable values that get replaced,
 * so a snapshot can be written on another thread.
 */
class Library private constructor(
    playlists: List<Playlist>,
    history: History,
    private val playlistsFile: File?,
    private val historyFile: File?,
) {
    /** Nothing is ever saved. */
    constructor() : this(emptyList(), History(), null, null)

    var playlists: List<Playlist> = playlists
        private set
    var history: History = history
        private set

    val recs: List<Song> get() = history.recs.items
    val hasHistory: Boolean get() = history.songs.isNotEmpty()

    fun get(id: String): Playlist? = playlists.firstOrNull { it.id == id }

    /** Creates a playlist, or finds the one with the same name. Null when the name is empty. */
    fun create(name: String): String? {
        val n = clampChars(name, MAX_NAME_CHARS)
        if (n.isEmpty()) return null
        playlists.firstOrNull { it.name.lowercase() == n.lowercase() }?.let { return it.id }
        val id = UUID.randomUUID().toString()
        playlists = playlists + Playlist(id, n)
        return id
    }

    fun rename(id: String, name: String) {
        val n = clampChars(name, MAX_NAME_CHARS)
        if (n.isNotEmpty()) update(id) { it.copy(name = n) }
    }

    fun delete(id: String) {
        playlists = playlists.filter { it.id != id }
    }

    /** Adds songs, skipping the ones already there. Null when the playlist no longer exists. */
    fun add(id: String, songs: List<Song>): Added? {
        val p = get(id) ?: return null
        val have = p.tracks.mapTo(HashSet()) { it.youtubeId }
        val tracks = ArrayList(p.tracks)
        var noRoom = 0
        for (s in songs) {
            if (!YouTubeUrl.isValidVideoId(s.youtubeId) || s.youtubeId in have) continue
            if (tracks.size >= MAX_PLAYLIST) {
                noRoom++
                continue
            }
            have += s.youtubeId
            tracks += s.copy(title = clampChars(s.title, MAX_TITLE_CHARS))
        }
        val added = tracks.size - p.tracks.size
        if (added > 0) update(id) { it.copy(tracks = tracks) }
        return Added(added, noRoom)
    }

    fun remove(id: String, youtubeId: String) =
        update(id) { p -> p.copy(tracks = p.tracks.filter { it.youtubeId != youtubeId }) }

    /** Moves a song to [to] (clamped to the list). */
    fun moveSong(id: String, youtubeId: String, to: Int) = update(id) { p ->
        val from = p.tracks.indexOfFirst { it.youtubeId == youtubeId }
        if (from < 0) return@update p
        val t = ArrayList(p.tracks)
        val s = t.removeAt(from)
        t.add(to.coerceIn(0, t.size), s)
        p.copy(tracks = t)
    }

    /** Fills in durations that were unknown when songs were saved. True when something changed. */
    fun learnDurations(queue: List<Track>): Boolean {
        if (playlists.none { p -> p.tracks.any { it.durationMs == null } }) return false
        val known = HashMap<String, Long>()
        for (t in queue) t.durationMs?.let { known[t.youtubeId] = it }
        var changed = false
        val out = playlists.map { p ->
            p.copy(
                tracks = p.tracks.map { t ->
                    val d = if (t.durationMs == null) known[t.youtubeId] else null
                    if (d == null) t else t.copy(durationMs = d).also { changed = true }
                },
            )
        }
        if (changed) playlists = out
        return changed
    }

    /** The song was listened to. [own]: this user added it to the room. */
    fun record(song: Song, own: Boolean, now: Long) {
        val h = history.songs[song.youtubeId] ?: Heard()
        val songs = HashMap(history.songs)
        songs[song.youtubeId] = Heard(
            title = song.title,
            durationMs = song.durationMs ?: h.durationMs,
            plays = h.plays + 1,
            ownPlays = h.ownPlays + if (own) 1 else 0,
            score = h.scoreAt(now) + if (own) HEARD_OWN else HEARD,
            lastUnix = now,
        )
        if (songs.size > MAX_HISTORY) {
            // The weakest ones are forgotten.
            val scores = songs.values.map { it.scoreAt(now) }.sorted()
            val cut = scores[scores.size - MAX_HISTORY * 9 / 10]
            songs.values.retainAll { it.scoreAt(now) >= cut }
        }
        history = history.copy(songs = songs)
    }

    /** Forgets what was listened to (and the recommendations made from it). */
    fun clearHistory() {
        history = History()
    }

    fun setRecs(items: List<Song>, now: Long) {
        history = history.copy(recs = Recs(now, items))
    }

    fun recsStale(now: Long): Boolean =
        recs.isEmpty() || history.recs.atUnix > now || now - history.recs.atUnix > RECS_TTL_SECS

    /**
     * Songs to look for similar ones to: the three favourites, and two more
     * drawn at random by score so a refresh brings something new. Without
     * history or playlists, songs of the room's queue.
     */
    fun seeds(queue: List<Track>, now: Long, random: () -> Double = { Random.nextDouble() }): List<Seed> {
        val pool = LinkedHashMap<String, Seed>()
        for ((id, h) in history.songs) pool[id] = Seed(Song(id, h.title, h.durationMs), h.scoreAt(now))
        val saved = HashSet<String>()
        for (t in playlists.flatMap { it.tracks }) {
            if (!saved.add(t.youtubeId)) continue
            val s = pool[t.youtubeId] ?: Seed(t, 0.0)
            pool[t.youtubeId] = s.copy(weight = s.weight + IN_PLAYLIST)
        }
        if (pool.isEmpty()) for (t in queue) pool[t.youtubeId] = Seed(t.toSong(), 1.0)
        // Equal scores (a fresh playlist) come in a random order.
        val sorted = pool.values
            .filter { YouTubeUrl.isValidVideoId(it.song.youtubeId) && it.weight > 0.0 }
            .map { it to random() }
            .sortedWith(compareByDescending<Pair<Seed, Double>> { it.first.weight }.thenBy { it.second })
            .map { it.first }
        val out = ArrayList(sorted.take(TOP_SEEDS))
        val rest = ArrayList(sorted.drop(TOP_SEEDS))
        repeat(RANDOM_SEEDS) {
            if (rest.isEmpty()) return out
            var at = random() * rest.sumOf { it.weight }
            val pick = rest.indexOfFirst { at -= it.weight; at < 0.0 }
            out += rest.removeAt(if (pick >= 0) pick else rest.size - 1)
        }
        return out
    }

    /**
     * Orders the songs of the seeds' mixes; one that shows up in several mixes
     * rises. Leaves out what the user already has or just heard.
     */
    fun rank(seeds: List<Seed>, mixes: List<Mix>, queue: List<Track>, now: Long): List<Song> {
        val ids = HashSet<String>()
        val titles = HashSet<String>()
        fun skip(id: String, title: String) {
            ids += id
            titles += titleKey(title)
        }
        for (t in queue) skip(t.youtubeId, t.title)
        for (p in playlists) for (t in p.tracks) skip(t.youtubeId, t.title)
        for (s in seeds) skip(s.song.youtubeId, s.song.title)
        for ((id, h) in history.songs) if (now - h.lastUnix < RECENT_SECS) skip(id, h.title)

        val songs = LinkedHashMap<String, Song>()
        val scores = HashMap<String, Double>()
        for (m in mixes) {
            m.songs.forEachIndexed { i, s ->
                val lengthOk = s.durationMs == null || s.durationMs in MIN_REC_MS..MAX_REC_MS
                if (!lengthOk || s.youtubeId in ids) return@forEachIndexed
                songs.putIfAbsent(s.youtubeId, s)
                scores[s.youtubeId] = (scores[s.youtubeId] ?: 0.0) + m.weight / (i + 3.0)
            }
        }
        return songs.values
            .sortedWith(compareByDescending<Song> { scores.getValue(it.youtubeId) }.thenBy { it.youtubeId })
            .filter { titles.add(titleKey(it.title)) }
            .take(MAX_RECS)
    }

    /** Writes a snapshot of [playlists]; safe on any thread, one call at a time. */
    @Throws(IOException::class)
    fun savePlaylists(snapshot: List<Playlist> = playlists) =
        write(playlistsFile, ListSerializer(Playlist.serializer()), snapshot)

    /** Writes a snapshot of [history]; safe on any thread, one call at a time. */
    @Throws(IOException::class)
    fun saveHistory(snapshot: History = history) = write(historyFile, History.serializer(), snapshot)

    private inline fun update(id: String, f: (Playlist) -> Playlist) {
        playlists = playlists.map { if (it.id == id) f(it) else it }
    }

    companion object {
        /** Longest playlist name, in characters. */
        const val MAX_NAME_CHARS = 60
        private const val MAX_TITLE_CHARS = 200
        /** Most songs in one playlist (a room's queue holds no more). */
        const val MAX_PLAYLIST = Resolver.MAX_QUEUE
        /** How many recommendations are shown. */
        const val MAX_RECS = 30
        const val MAX_HISTORY = 3000
        const val HALF_LIFE_SECS = 14.0 * 24 * 3600
        private const val RECS_TTL_SECS = 12L * 3600
        private const val RECENT_SECS = 7L * 24 * 3600
        private const val HEARD = 1.0
        private const val HEARD_OWN = 2.5
        private const val IN_PLAYLIST = 2.0
        private const val TOP_SEEDS = 3
        private const val RANDOM_SEEDS = 2
        private const val MIN_REC_MS = 60_000L
        private const val MAX_REC_MS = 12 * 60_000L
        /** JSON size of one AddResolved batch (the server takes 64 KB messages). */
        private const val BATCH_BYTES = 40 * 1024

        private val json = Json {
            ignoreUnknownKeys = true
            encodeDefaults = true
        }

        fun unixNow(): Long = System.currentTimeMillis() / 1000

        /** Loads both files. Without a directory nothing is ever saved. */
        fun load(dir: File?): Library {
            if (dir == null) return Library()
            val (lists, listsFile) = read(File(dir, "playlists.json"), ListSerializer(Playlist.serializer()), emptyList())
            val (history, historyFile) = read(File(dir, "history.json"), History.serializer(), History())
            // The lists on screen need unique ids, also after a hand edit.
            return Library(
                lists.distinctBy { it.id }.map { p -> p.copy(tracks = p.tracks.distinctBy { it.youtubeId }) },
                history.copy(recs = history.recs.copy(items = history.recs.items.distinctBy { it.youtubeId })),
                listsFile,
                historyFile,
            )
        }

        /**
         * Reads a JSON file. Unreadable content is set aside as `.bad`, so it
         * is never overwritten; a file that cannot be set aside is not saved to.
         */
        private fun <T> read(file: File, s: KSerializer<T>, empty: T): Pair<T, File?> {
            val text = try {
                if (!file.exists()) return empty to file
                file.readText().removePrefix("﻿")
            } catch (e: IOException) {
                return empty to null
            }
            return try {
                json.decodeFromString(s, text) to file
            } catch (e: SerializationException) {
                empty to setAside(file)
            } catch (e: IllegalArgumentException) {
                empty to setAside(file)
            }
        }

        private fun setAside(file: File): File? {
            var bad = File(file.path + ".bad")
            if (bad.exists()) bad = File("${file.path}.${System.currentTimeMillis()}.bad")
            return try {
                Files.move(file.toPath(), bad.toPath())
                file
            } catch (e: IOException) {
                null
            }
        }

        private fun <T> write(file: File?, s: KSerializer<T>, value: T) {
            if (file == null) return
            val bytes = json.encodeToString(s, value).toByteArray()
            file.parentFile?.mkdirs()
            val tmp = File(file.path + ".tmp")
            FileOutputStream(tmp).use {
                it.write(bytes)
                it.fd.sync()
            }
            Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        }

        private fun clampChars(s: String, max: Int): String {
            val t = s.trim()
            if (t.codePointCount(0, t.length) <= max) return t
            return t.substring(0, t.offsetByCodePoints(0, max)).trim()
        }

        /** "Song (Official Video) [4K]" and "song" are the same song. */
        fun titleKey(title: String): String {
            val low = title.lowercase()
            var depth = 0
            val out = StringBuilder()
            for (c in low) {
                when {
                    c == '(' || c == '[' -> depth++
                    c == ')' || c == ']' -> depth = (depth - 1).coerceAtLeast(0)
                    depth == 0 -> out.append(c)
                }
            }
            return out.split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ").ifEmpty { low }
        }

        /** Splits songs into batches that fit in one message to the server. */
        fun batches(songs: List<Song>): List<List<TrackInfo>> {
            val out = ArrayList<ArrayList<TrackInfo>>()
            var size = 0
            for (s in songs) {
                val t = TrackInfo(s.youtubeId, s.title, s.durationMs)
                val len = Wire.json.encodeToString(TrackInfo.serializer(), t).toByteArray().size + 1
                if (out.isEmpty() || size + len > BATCH_BYTES) {
                    out += ArrayList<TrackInfo>()
                    size = 0
                }
                out.last() += t
                size += len
            }
            return out
        }
    }
}

/** Decides when a playing song counts as listened to. */
class ListenTimer {
    private var track: String? = null
    private var counted = false
    private var last: Long? = null
    var heardMs = 0L
        private set

    /** Call often. True once per play, when enough was heard. */
    fun tick(track: String?, playing: Boolean, durationMs: Long?, nowMs: Long): Boolean {
        if (track != this.track) {
            this.track = track
            heardMs = 0
            counted = false
            last = null
        }
        val before = last
        last = null
        if (track == null || !playing) return false
        last = nowMs
        if (before != null) heardMs += (nowMs - before).coerceIn(0, MAX_STEP_MS)
        val need = if (durationMs != null) minOf(LISTEN_MS, durationMs / 2) else LISTEN_MS
        if (counted || heardMs < need) return false
        counted = true
        return true
    }

    companion object {
        private const val LISTEN_MS = 30_000L
        const val MAX_STEP_MS = 2_000L
    }
}
