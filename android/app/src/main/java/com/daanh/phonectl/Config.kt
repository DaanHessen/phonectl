package com.daanh.phonectl

import android.content.Context
import java.util.Base64

/**
 * Pairing data, set once from the laptop's `phonectl setup` string:
 *
 *   phonectl:1;host=100.101.102.103;port=47201;bt=00:11:22:33:44:55;name=omarchy;key=<base64>
 *
 * Lives in app-private storage (not readable by other apps).
 */
class Config(val host: String, val port: Int, val bt: String?, val name: String, val key: ByteArray) {
    companion object {
        private const val PREFS = "pairing"

        fun load(context: Context): Config? {
            val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val host = p.getString("host", null) ?: return null
            val key = p.getString("key", null) ?: return null
            return Config(
                host,
                p.getInt("port", Protocol.TCP_PORT),
                p.getString("bt", null),
                p.getString("name", "laptop")!!,
                Base64.getDecoder().decode(key),
            )
        }

        /** Parses and stores a pairing string. Returns null when it is malformed. */
        fun pair(context: Context, text: String): Config? {
            val config = parse(text) ?: return null
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString("host", config.host)
                .putInt("port", config.port)
                .putString("bt", config.bt)
                .putString("name", config.name)
                .putString("key", Base64.getEncoder().encodeToString(config.key))
                .apply()
            return config
        }

        fun parse(text: String): Config? {
            val trimmed = text.trim()
            if (!trimmed.startsWith("phonectl:1;")) return null
            val fields = trimmed.removePrefix("phonectl:1;").split(';')
                .mapNotNull { part -> part.indexOf('=').takeIf { it > 0 }?.let { part.substring(0, it) to part.substring(it + 1) } }
                .toMap()
            val host = fields["host"]?.takeIf { it.isNotBlank() } ?: return null
            val key = try {
                Base64.getDecoder().decode(fields["key"] ?: return null)
            } catch (_: IllegalArgumentException) {
                return null
            }
            if (key.size < 16) return null
            val bt = fields["bt"]?.uppercase()?.takeIf { Regex("([0-9A-F]{2}:){5}[0-9A-F]{2}").matches(it) }
            return Config(host, fields["port"]?.toIntOrNull() ?: Protocol.TCP_PORT, bt, fields["name"] ?: "laptop", key)
        }
    }
}
