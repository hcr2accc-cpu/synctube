// Saved settings (connect form, volume, recent rooms).
package rs.synctube

import android.content.Context

class Prefs(context: Context) {
    private val p = context.getSharedPreferences("synctube", Context.MODE_PRIVATE)

    var serverUrl: String
        get() = p.getString("server", null)?.takeIf { it.isNotBlank() } ?: BuildConfig.DEFAULT_SERVER_URL
        set(v) = p.edit().putString("server", v).apply()

    var room: String
        get() = p.getString("room", "") ?: ""
        set(v) = p.edit().putString("room", v).apply()

    var nickname: String
        get() = p.getString("nick", "") ?: ""
        set(v) = p.edit().putString("nick", v).apply()

    var password: String
        get() = p.getString("password", "") ?: ""
        set(v) = p.edit().putString("password", v).apply()

    var volume: Int
        get() = p.getInt("volume", 100)
        set(v) = p.edit().putInt("volume", v).apply()

    var lowLatency: Boolean
        get() = p.getBoolean("low_latency", false)
        set(v) = p.edit().putBoolean("low_latency", v).apply()

    var audioOffsetMs: Int
        get() = p.getInt("audio_offset", 0).coerceIn(-500, 500)
        set(v) = p.edit().putInt("audio_offset", v).apply()

    var recentRooms: List<String>
        get() = (p.getString("recent", "") ?: "").split('\n').filter { it.isNotBlank() }
        set(v) = p.edit().putString("recent", v.joinToString("\n")).apply()

    fun rememberRoom(room: String) {
        recentRooms = (listOf(room) + recentRooms.filter { !it.equals(room, ignoreCase = true) }).take(6)
    }
}
