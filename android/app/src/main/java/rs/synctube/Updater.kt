// In-app update: newest vX.Y release on GitHub with synctube.apk -> download
// -> Android's installer. The server is not involved.
package rs.synctube

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.core.content.FileProvider
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File

data class AppUpdate(val tag: String, val url: String, val size: Long?)

sealed class UpdatePhase {
    data object Checking : UpdatePhase()
    data object UpToDate : UpdatePhase()
    data class Available(val update: AppUpdate) : UpdatePhase()
    data class Downloading(val tag: String, val done: Long, val total: Long?) : UpdatePhase()
    data class NeedsPermission(val update: AppUpdate) : UpdatePhase()
    data class Failed(val message: String, val update: AppUpdate?) : UpdatePhase()
}

class Updater(private val ctx: Context, private val http: OkHttpClient) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val _phase = MutableStateFlow<UpdatePhase>(UpdatePhase.Checking)
    val phase: StateFlow<UpdatePhase> = _phase
    private var checked = false

    /** Asks GitHub once per process. */
    fun check() {
        if (checked) return
        checked = true
        scope.launch {
            _phase.value = runCatching { withContext(Dispatchers.IO) { fetch() } }
                .onFailure { Log.i(TAG, "update check failed", it) }
                .getOrNull()
                ?.let { UpdatePhase.Available(it) }
                ?: UpdatePhase.UpToDate
        }
    }

    private fun fetch(): AppUpdate? {
        val req = Request.Builder().url(RELEASES_API).header("Accept", "application/vnd.github+json").build()
        val body = http.newCall(req).execute().use { r ->
            if (!r.isSuccessful) error("GitHub: ${r.code}")
            r.body?.string() ?: error("GitHub: empty")
        }
        return pick(Wire.json.parseToJsonElement(body), BuildConfig.VERSION_NAME, ASSET)
    }

    /** Downloads the APK and opens Android's installer for it. */
    fun install(u: AppUpdate) {
        if (_phase.value is UpdatePhase.Downloading) return
        if (Build.VERSION.SDK_INT >= 26 && !ctx.packageManager.canRequestPackageInstalls()) {
            _phase.value = UpdatePhase.NeedsPermission(u)
            ctx.startActivity(
                Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${ctx.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            return
        }
        _phase.value = UpdatePhase.Downloading(u.tag, 0, u.size)
        scope.launch {
            val apk = runCatching { withContext(Dispatchers.IO) { download(u) } }
            apk.onSuccess { file ->
                _phase.value = UpdatePhase.Available(u)
                val uri = FileProvider.getUriForFile(ctx, "${ctx.packageName}.updates", file)
                ctx.startActivity(
                    Intent(Intent.ACTION_VIEW)
                        .setDataAndType(uri, "application/vnd.android.package-archive")
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }.onFailure {
                Log.e(TAG, "update download failed", it)
                _phase.value = UpdatePhase.Failed(it.message ?: it.toString(), u)
            }
        }
    }

    private fun download(u: AppUpdate): File {
        val dir = File(ctx.cacheDir, "updates").apply { mkdirs() }
        dir.listFiles()?.forEach { it.delete() }
        val part = File(dir, "synctube.apk.part")
        val dest = File(dir, "synctube.apk")
        http.newCall(Request.Builder().url(u.url).build()).execute().use { r ->
            if (!r.isSuccessful) error("GitHub: ${r.code}")
            val body = r.body ?: error("GitHub: empty")
            val total = u.size ?: body.contentLength().takeIf { it > 0 }
            var done = 0L
            var shown = 0L
            body.byteStream().use { input ->
                part.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        done += n
                        if (done - shown >= 256 * 1024) {
                            shown = done
                            _phase.value = UpdatePhase.Downloading(u.tag, done, total)
                        }
                    }
                }
            }
            if (done == 0L || (total != null && total != done)) error("incomplete download")
        }
        if (!part.renameTo(dest)) error("cannot save the download")
        return dest
    }

    companion object {
        private const val TAG = "Updater"
        private const val RELEASES_API = "https://api.github.com/repos/hcr2accc-cpu/synctube/releases?per_page=20"
        const val RELEASES_PAGE = "https://github.com/hcr2accc-cpu/synctube/releases/latest"
        const val ASSET = "synctube.apk"

        /** `v0.7`, `0.7.1` -> [0, 7, 1]; anything else (`windows`, `v1-beta`) is null. */
        fun parseVersion(s: String): List<Long>? {
            val parts = s.removePrefix("v").split('.')
            if (parts.size !in 2..3 || parts.any { p -> p.isEmpty() || !p.all { it.isDigit() } }) return null
            return parts.map { it.toLong() } + if (parts.size == 2) listOf(0L) else emptyList()
        }

        private fun compare(a: List<Long>, b: List<Long>): Int =
            a.zip(b).map { (x, y) -> x.compareTo(y) }.firstOrNull { it != 0 } ?: 0

        /** The newest proper release above [current] that has [asset]. */
        fun pick(releases: JsonElement, current: String, asset: String): AppUpdate? {
            val cur = parseVersion(current) ?: return null
            val list = releases as? JsonArray ?: return null
            return list.mapNotNull { r ->
                val o = r as? JsonObject ?: return@mapNotNull null
                if (o.bool("draft") || o.bool("prerelease")) return@mapNotNull null
                val tag = o.str("tag_name") ?: return@mapNotNull null
                val v = parseVersion(tag) ?: return@mapNotNull null
                val a = (o["assets"] as? JsonArray)?.mapNotNull { it as? JsonObject }
                    ?.firstOrNull { it.str("name") == asset } ?: return@mapNotNull null
                val url = a.str("browser_download_url") ?: return@mapNotNull null
                v to AppUpdate(tag, url, a["size"]?.jsonPrimitive?.longOrNull)
            }
                .filter { compare(it.first, cur) > 0 }
                .maxWithOrNull { x, y -> compare(x.first, y.first) }
                ?.second
        }

        private fun JsonObject.bool(k: String) = this[k]?.jsonPrimitive?.booleanOrNull ?: false
        private fun JsonObject.str(k: String) = (this[k] as? kotlinx.serialization.json.JsonPrimitive)
            ?.takeIf { it.isString }?.content
    }
}
