// YouTube metadata, search and audio stream URLs via NewPipeExtractor
// (the Android stand-in for yt-dlp).
package rs.synctube

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.RequestBody.Companion.toRequestBody
import org.schabi.newpipe.extractor.MediaFormat
import org.schabi.newpipe.extractor.NewPipe
import org.schabi.newpipe.extractor.ServiceList
import org.schabi.newpipe.extractor.downloader.Downloader
import org.schabi.newpipe.extractor.downloader.Request
import org.schabi.newpipe.extractor.downloader.Response
import org.schabi.newpipe.extractor.exceptions.ReCaptchaException
import org.schabi.newpipe.extractor.playlist.PlaylistInfo
import org.schabi.newpipe.extractor.playlist.PlaylistInfoItem
import org.schabi.newpipe.extractor.search.SearchInfo
import org.schabi.newpipe.extractor.services.youtube.YoutubeParsingHelper
import org.schabi.newpipe.extractor.services.youtube.linkHandler.YoutubeSearchQueryHandlerFactory
import org.schabi.newpipe.extractor.stream.AudioStream
import org.schabi.newpipe.extractor.stream.AudioTrackType
import org.schabi.newpipe.extractor.stream.DeliveryMethod
import org.schabi.newpipe.extractor.stream.StreamInfo
import org.schabi.newpipe.extractor.stream.StreamInfoItem
import org.schabi.newpipe.extractor.stream.StreamType
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

data class StreamUrl(val url: String, val durationMs: Long?)

data class SearchHit(
    val youtubeId: String?,
    val playlistId: String?,
    val title: String,
    val uploader: String,
    val durationMs: Long?,
    val count: Long?,
    val thumb: String?,
)

class Resolver(private val http: OkHttpClient) {
    private class Cached(val v: StreamUrl, val at: Long)
    private val streams = ConcurrentHashMap<String, Cached>()
    private val titles = ConcurrentHashMap<String, TrackInfo>()
    /** Background prefetches run one at a time so they don't slow down the current song. */
    private val background = Semaphore(1)

    init {
        initNewPipe(http)
    }

    fun invalidate(id: String) {
        streams.remove(id)
    }

    fun cachedStream(id: String): StreamUrl? =
        streams[id]?.takeIf { System.currentTimeMillis() - it.at < CACHE_MS }?.v

    /** Best audio stream for a video (cached for an hour). */
    suspend fun stream(id: String): StreamUrl = withContext(Dispatchers.IO) {
        cachedStream(id) ?: fetch(id)
    }

    suspend fun prefetch(id: String) {
        if (cachedStream(id) != null) return
        background.withPermit {
            if (cachedStream(id) == null) runCatching { stream(id) }
        }
    }

    private fun fetch(id: String): StreamUrl {
        val info = StreamInfo.getInfo(ServiceList.YouTube, YouTubeUrl.watchUrl(id))
        val durationMs = info.duration.takeIf { it > 0 }?.times(1000)
        titles[id] = TrackInfo(id, info.name ?: id, durationMs)
        val best = pickAudio(info.audioStreams) ?: throw IllegalStateException("no audio stream")
        val s = StreamUrl(best.content, durationMs)
        streams[id] = Cached(s, System.currentTimeMillis())
        return s
    }

    /** Metadata for a video or a whole playlist (up to the queue limit). */
    suspend fun metadata(r: YoutubeRef): List<TrackInfo> = withContext(Dispatchers.IO) {
        when (r) {
            is YoutubeRef.Video -> {
                titles[r.id]?.let { return@withContext listOf(it) }
                fetch(r.id)
                listOf(titles[r.id] ?: TrackInfo(r.id, r.id, null))
            }
            is YoutubeRef.Playlist -> {
                val service = ServiceList.YouTube
                val url = YouTubeUrl.playlistUrl(r.id)
                val info = PlaylistInfo.getInfo(service, url)
                val out = ArrayList<TrackInfo>()
                fun take(items: List<StreamInfoItem>) {
                    for (it in items) {
                        val ref = YouTubeUrl.parse(it.url) as? YoutubeRef.Video ?: continue
                        out.add(TrackInfo(ref.id, it.name ?: ref.id, it.duration.takeIf { d -> d > 0 }?.times(1000)))
                    }
                }
                take(info.relatedItems)
                var page = info.nextPage
                while (page != null && out.size < MAX_QUEUE) {
                    val more = PlaylistInfo.getMoreItems(service, url, page)
                    take(more.items)
                    page = if (more.hasNextPage()) more.nextPage else null
                }
                out.take(MAX_QUEUE)
            }
        }
    }

    suspend fun search(query: String, playlists: Boolean): List<SearchHit> = withContext(Dispatchers.IO) {
        val service = ServiceList.YouTube
        val filter = if (playlists) YoutubeSearchQueryHandlerFactory.PLAYLISTS else YoutubeSearchQueryHandlerFactory.VIDEOS
        val handler = service.searchQHFactory.fromQuery(query, listOf(filter), "")
        val info = SearchInfo.getInfo(service, handler)
        info.relatedItems.mapNotNull { item ->
            when (item) {
                is StreamInfoItem -> {
                    val ref = YouTubeUrl.parse(item.url) as? YoutubeRef.Video ?: return@mapNotNull null
                    SearchHit(
                        youtubeId = ref.id, playlistId = null, title = item.name ?: ref.id,
                        uploader = item.uploaderName ?: "",
                        durationMs = item.duration.takeIf { it > 0 }?.times(1000),
                        count = null, thumb = YouTubeUrl.thumbUrl(ref.id),
                    )
                }
                is PlaylistInfoItem -> {
                    val ref = YouTubeUrl.parse(item.url) as? YoutubeRef.Playlist ?: return@mapNotNull null
                    SearchHit(
                        youtubeId = null, playlistId = ref.id, title = item.name ?: ref.id,
                        uploader = item.uploaderName ?: "", durationMs = null,
                        count = item.streamCount.takeIf { it >= 0 },
                        thumb = item.thumbnails.maxByOrNull { it.width }?.url,
                    )
                }
                else -> null
            }
        }
    }

    /**
     * YouTube's mix for a song: similar songs, the song itself first. Waits
     * behind other background work.
     */
    suspend fun mix(id: String): List<Song> = background.withPermit {
        withContext(Dispatchers.IO) {
            runCatching { mixPlaylist(id) }.getOrNull()?.takeIf { it.size > 1 } ?: related(id)
        }
    }

    internal fun mixPlaylist(id: String): List<Song> {
        val info = PlaylistInfo.getInfo(ServiceList.YouTube, "${YouTubeUrl.watchUrl(id)}&list=RD$id")
        return mixSongs(info.relatedItems)
    }

    /** What YouTube shows next to the video, when there is no mix. */
    internal fun related(id: String): List<Song> {
        val info = StreamInfo.getInfo(ServiceList.YouTube, YouTubeUrl.watchUrl(id))
        val self = Song(id, info.name ?: id, info.duration.takeIf { it > 0 }?.times(1000))
        return listOf(self) + mixSongs(info.relatedItems.filterIsInstance<StreamInfoItem>())
    }

    private fun mixSongs(items: List<StreamInfoItem>): List<Song> = items.mapNotNull {
        // Live streams never end, so they cannot be queued.
        if (it.streamType == StreamType.LIVE_STREAM || it.streamType == StreamType.AUDIO_LIVE_STREAM) return@mapNotNull null
        val ref = YouTubeUrl.parse(it.url) as? YoutubeRef.Video ?: return@mapNotNull null
        Song(
            ref.id,
            it.name?.trim()?.ifEmpty { null } ?: return@mapNotNull null,
            it.duration.takeIf { d -> d > 0 }?.times(1000),
            it.uploaderName?.trim()?.ifEmpty { null },
        )
    }.take(MIX_SONGS)

    companion object {
        const val MAX_QUEUE = 500
        /** How many songs of a YouTube mix are looked at. */
        private const val MIX_SONGS = 25
        private const val CACHE_MS = 60 * 60 * 1000L
        const val USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0"

        @Volatile private var initialized = false

        fun initNewPipe(http: OkHttpClient) {
            synchronized(this) {
                if (initialized) return
                NewPipe.init(OkDownloader(http))
                initialized = true
            }
        }

        /** Highest-bitrate original-language audio, Opus preferred (what yt-dlp's bestaudio picks). */
        fun pickAudio(list: List<AudioStream>): AudioStream? {
            val usable = list.filter { it.deliveryMethod == DeliveryMethod.PROGRESSIVE_HTTP && it.isUrl }
            val original = usable.filter {
                it.audioTrackType == null || it.audioTrackType == AudioTrackType.ORIGINAL
            }.ifEmpty { usable }
            fun rate(a: AudioStream) = if (a.averageBitrate > 0) a.averageBitrate else a.bitrate
            return original.filter { it.format == MediaFormat.WEBMA_OPUS }.maxByOrNull(::rate)
                ?: original.filter { it.format == MediaFormat.M4A }.maxByOrNull(::rate)
                ?: original.maxByOrNull(::rate)
        }

        /** Headers YouTube expects on googlevideo requests for a given stream URL. */
        fun streamHeaders(url: String): Map<String, String> {
            val ua = if (YoutubeParsingHelper.isVisionOsStreamingUrl(url)) {
                YoutubeParsingHelper.getVisionOsUserAgent(null)
            } else {
                USER_AGENT
            }
            val h = HashMap<String, String>()
            h["User-Agent"] = ua
            if (YoutubeParsingHelper.isWebStreamingUrl(url)) {
                h["Origin"] = "https://www.youtube.com"
                h["Referer"] = "https://www.youtube.com/"
            }
            return h
        }
    }
}

/** NewPipe's HTTP layer on OkHttp. */
class OkDownloader(base: OkHttpClient) : Downloader() {
    private val client = base.newBuilder().readTimeout(30, TimeUnit.SECONDS).build()

    override fun execute(request: Request): Response {
        val body = request.dataToSend()?.toRequestBody()
        val b = okhttp3.Request.Builder()
            .method(request.httpMethod(), body)
            .url(request.url())
            .addHeader("User-Agent", Resolver.USER_AGENT)
        for ((name, values) in request.headers()) {
            b.removeHeader(name)
            for (v in values) b.addHeader(name, v)
        }
        client.newCall(b.build()).execute().use { resp ->
            if (resp.code == 429) throw ReCaptchaException("reCaptcha Challenge requested", request.url())
            return Response(
                resp.code, resp.message, resp.headers.toMultimap(),
                resp.body?.string(), resp.request.url.toString(),
            )
        }
    }
}
