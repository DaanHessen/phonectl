package com.daanh.phonectl

import android.app.KeyguardManager
import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale

/**
 * The gate every laptop call passes before it reaches a feature: only the
 * methods in [RULES] exist, each has a rate limit, harmful ones need the phone
 * unlocked, and every call (allowed or refused) lands in "Recent laptop
 * actions" in the app. Unknown methods are refused here (fail closed).
 */
object Actions {
    enum class Risk { READ, NORMAL, HARMFUL }

    class Rule(val label: String, val perMinute: Int, val risk: Risk)

    val RULES: Map<String, Rule> = mapOf(
        "diag" to Rule("read the app log", 20, Risk.READ),
        "icon" to Rule("fetch an app icon", 120, Risk.READ),
        "ring" to Rule("ring the phone", 6, Risk.NORMAL),
        "ringer" to Rule("change the ringer mode", 20, Risk.NORMAL),
        "media_action" to Rule("media control", 60, Risk.NORMAL),
        "test_notification" to Rule("post a test notification", 6, Risk.NORMAL),
        "notification_action" to Rule("tap a notification action", 30, Risk.NORMAL),
        "notification_open" to Rule("open a notification", 30, Risk.NORMAL),
        "notification_dismiss" to Rule("dismiss a notification", 60, Risk.NORMAL),
        "adb_open" to Rule("open wireless debugging to start the clipboard watcher", 3, Risk.NORMAL),
        "adb_close" to Rule("close wireless debugging", 10, Risk.NORMAL),
        "store_check" to Rule("check the phone store for updates", 6, Risk.NORMAL),
        "install_update" to Rule("install a phonectl update (own signature, newer only)", 3, Risk.NORMAL),
        "type_text" to Rule("type text", 30, Risk.HARMFUL),
    )

    private val limiter = RateLimiter { RULES[it]?.perMinute ?: 0 }
    private val log = RecentLog(100)
    private var file: File? = null
    private var keyguard: KeyguardManager? = null
    @Volatile var onChange: (() -> Unit)? = null

    fun init(context: Context) {
        keyguard = context.getSystemService(KeyguardManager::class.java)
        file = File(context.filesDir, "recent-actions.json").also { f ->
            try {
                if (f.exists()) log.load(JSONArray(f.readText()))
            } catch (_: Exception) {
                f.delete()
            }
        }
    }

    /** Null when [method] may run now, otherwise the reason it is refused (already recorded). */
    fun admit(method: String, now: Long = System.currentTimeMillis()): String? {
        val rule = RULES[method]
        val refusal = when {
            rule == null -> "unknown method"
            !limiter.allow(method, now) -> "rate limit (${rule.perMinute}/min)"
            rule.risk == Risk.HARMFUL && keyguard?.isDeviceLocked != false -> "phone is locked"
            else -> null
        }
        if (refusal != null) record(method, "refused: $refusal", now)
        return refusal
    }

    fun record(method: String, outcome: String, now: Long = System.currentTimeMillis()) {
        val label = RULES[method]?.label ?: method.filter { it.isLetterOrDigit() || it == '_' }.take(40)
        log.add(now, method, label, outcome.take(120))
        Diag.log("laptop: $method $outcome".take(160))
        save()
        onChange?.invoke()
    }

    /** Something the phone did on its own behalf (store installs), shown with the laptop's actions. */
    fun note(key: String, label: String, outcome: String, now: Long = System.currentTimeMillis()) {
        log.add(now, key, label, outcome.take(120))
        save()
        onChange?.invoke()
    }

    fun recent(): List<RecentLog.Entry> = log.entries()

    private fun save() {
        val f = file ?: return
        try {
            val tmp = File(f.path + ".tmp")
            tmp.writeText(log.toJson().toString())
            tmp.renameTo(f)
        } catch (_: Exception) {
        }
    }
}

/** At most `limit(key)` events per key in any 60 seconds. */
class RateLimiter(private val limit: (String) -> Int) {
    private val times = HashMap<String, ArrayDeque<Long>>()

    @Synchronized fun allow(key: String, now: Long): Boolean {
        val max = limit(key)
        if (max <= 0) return false
        val q = times.getOrPut(key) { ArrayDeque() }
        while (q.isNotEmpty() && now - q.peekFirst()!! >= 60_000L) q.removeFirst()
        if (q.size >= max) return false
        q.addLast(now)
        return true
    }
}

/** Newest-last list of laptop actions; repeats within a minute fold into one line with a count. */
class RecentLog(private val max: Int) {
    class Entry(val time: Long, val method: String, val label: String, val outcome: String, var count: Int) {
        fun line(): String {
            val clock = SimpleDateFormat("d MMM HH:mm:ss", Locale.ROOT).format(Date(time))
            val times = if (count > 1) " (${count}x)" else ""
            return "$clock  $label$times: $outcome"
        }
    }

    private val entries = ArrayDeque<Entry>()

    @Synchronized fun add(time: Long, method: String, label: String, outcome: String) {
        val last = entries.peekLast()
        if (last != null && last.method == method && last.label == label && last.outcome == outcome && time - last.time < 60_000L) {
            last.count++
            return
        }
        entries.addLast(Entry(time, method, label, outcome, 1))
        while (entries.size > max) entries.removeFirst()
    }

    @Synchronized fun entries(): List<Entry> = entries.toList()

    @Synchronized fun toJson(): JSONArray = JSONArray().apply {
        for (e in entries) put(JSONObject().put("t", e.time).put("m", e.method).put("l", e.label).put("o", e.outcome).put("n", e.count))
    }

    @Synchronized fun load(array: JSONArray) {
        entries.clear()
        for (i in 0 until array.length()) {
            val o = array.getJSONObject(i)
            entries.addLast(Entry(o.getLong("t"), o.getString("m"), o.getString("l"), o.getString("o"), o.optInt("n", 1)))
        }
        while (entries.size > max) entries.removeFirst()
    }
}
