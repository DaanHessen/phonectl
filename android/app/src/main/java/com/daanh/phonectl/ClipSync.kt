package com.daanh.phonectl

import android.Manifest
import android.app.ActivityOptions
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.PersistableBundle
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader

/**
 * Two-way clipboard sync.
 *
 * Laptop → phone is unrestricted: any app may *write* the clipboard.
 *
 * Phone → laptop is where Android 10+ gets in the way: only the focused app
 * (or the IME) may *read* the clipboard, and change listeners are not even
 * called for background apps. What ClipboardService does instead is log
 * "Denying clipboard access to <pkg>" for every registered listener it skips.
 * With READ_LOGS (granted once over ADB; it survives turning debugging off),
 * a blocked `logcat` stream filtered to that one tag tells us exactly when the
 * clipboard changed. We then bring up [ClipReadActivity], an invisible
 * activity that takes focus for a frame, reads the clip, and finishes. Starting
 * it from the background needs the "display over other apps" app-op (only
 * used as the background-activity-start exemption; no overlay is ever drawn).
 * This is the same mechanism KDE Connect uses on Android 10+.
 *
 * When either grant is missing, phone → laptop still works manually: the
 * share sheet ("phonectl"), the Quick Settings tile, or opening the app.
 *
 * Loop prevention: every clip is identified by a content hash. A clip we
 * receive is remembered as the current hash, so its echo (our own write
 * triggers the change log) is dropped. Each change also carries the time it
 * was made; after a reconnect each side sends its latest clip and the newer
 * one wins, so a stale clip can never overwrite a fresh one.
 */
object ClipSync : Link.Feature {
    private const val TAG = "phonectl"
    private lateinit var context: Context
    private val handler get() = Link.handler

    /** Hash of the clip both sides are known to have (or we last saw). */
    private var currentHash: String? = null
    /** Time the current clip was made, on whichever side made it. */
    private var currentAt = 0L
    /** A local clip the laptop has not received yet. */
    private var pending: JSONObject? = null
    /** Ignore change notifications caused by our own writes until then. */
    private var suppressUntil = 0L
    private var watcher: Thread? = null

    fun start(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        val cm = context.getSystemService(ClipboardManager::class.java)
        // Registering is allowed in the background; callbacks only arrive while
        // we are focused, but registering is what makes ClipboardService log
        // the denial line we watch for.
        cm.addPrimaryClipChangedListener { readNow("listener") }
        startWatcher()
    }

    /**
     * Android 13+ adds a consent step on top of READ_LOGS: logd asks the user
     * ("Allow access to all device logs?") when the app is in the foreground,
     * and silently declines when it is not, handing back only our own lines.
     * So the watcher can only be armed from the foreground. After a reboot or
     * app update the process starts in the background, the probe sees no
     * system lines, and we post one quiet notification to re-arm with a tap.
     */
    @Volatile var logAccess = false
        private set
    @Volatile private var process: Process? = null

    fun autoAvailable(): Boolean =
        ::context.isInitialized && logAccess && Settings.canDrawOverlays(context)

    private fun hasReadLogs() = context.checkSelfPermission(Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED

    /** Starts the log watcher unless it is already running with system log access. */
    fun startWatcher() {
        if (!hasReadLogs()) return
        if (watcher?.isAlive == true && logAccess) return
        if (watcher?.isAlive == true) {
            // Running but declined (started in the background): restart now
            // that we are in the foreground and the consent prompt can show.
            process?.destroy()
            watcher?.interrupt()
        }
        watcher = Thread({ watch() }, "clip-logwatch").apply { isDaemon = true; start() }
    }

    /** True when logd gives us other processes' lines (consent granted). */
    private fun probe(): Boolean = try {
        // Declined access still returns our own uid's lines, so look for
        // system_server's (uid 1000), which always logs to this buffer.
        val p = ProcessBuilder("logcat", "-d", "-b", "system", "-t", "200", "-v", "brief,uid").redirectErrorStream(true).start()
        val text = p.inputStream.bufferedReader().readText()
        p.waitFor()
        val system = Regex("""^./[^(]*\(\s*(1000|system):""")
        text.lineSequence().any { system.containsMatchIn(it) }
    } catch (_: Exception) {
        false
    }

    private fun watch() {
        val needle = "Denying clipboard access to ${context.packageName},"
        var backoff = 5_000L
        while (!Thread.currentThread().isInterrupted) {
            val started = SystemClock.elapsedRealtime()
            logAccess = probe()
            StatusMonitor.refresh()
            Link.notifyListeners()
            if (!logAccess) {
                Diag.log("no system log access yet; automatic clipboard needs one tap")
                SetupNotice.clipboardNeedsTap(context)
                return
            }
            SetupNotice.clear(context)
            try {
                // -T 1: start at the newest entry (no backlog). The tag filter
                // runs in logcat; the process sleeps in read() between lines.
                val process = ProcessBuilder("logcat", "-b", "system", "-T", "1", "-v", "brief", "ClipboardService:E", "*:S")
                    .redirectErrorStream(true).start()
                this.process = process
                BufferedReader(InputStreamReader(process.inputStream)).use { reader ->
                    while (true) {
                        val line = reader.readLine() ?: break
                        if (line.contains(needle)) handler.post { onChangeDetected() }
                    }
                }
                process.destroy()
            } catch (e: Exception) {
                Log.w(TAG, "clipboard log watcher: ${e.javaClass.simpleName}")
            }
            // logcat only exits on its own when logd restarts or access is
            // revoked; back off so a revoked grant cannot spin.
            backoff = if (SystemClock.elapsedRealtime() - started > 60_000) 5_000 else (backoff * 2).coerceAtMost(30 * 60_000)
            try { Thread.sleep(backoff) } catch (_: InterruptedException) { return }
        }
    }

    private fun onChangeDetected() {
        if (SystemClock.elapsedRealtime() < suppressUntil) return
        if (!Settings.canDrawOverlays(context)) return
        try {
            val intent = Intent(context, ClipReadActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION or Intent.FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS)
            val options = ActivityOptions.makeBasic()
                .setPendingIntentBackgroundActivityStartMode(ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED)
            context.startActivity(intent, options.toBundle())
        } catch (e: Exception) {
            Log.w(TAG, "cannot start clipboard reader: ${e.javaClass.simpleName}")
        }
    }

    /** Reads the clipboard; only succeeds while one of our windows has focus. */
    fun readNow(source: String) {
        val cm = context.getSystemService(ClipboardManager::class.java)
        val clip = try { cm.primaryClip } catch (_: SecurityException) { null } ?: return
        val description = clip.description
        val extras: PersistableBundle? = description?.extras
        if (extras?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE) == true) return // passwords, OTPs
        if (extras?.getBoolean(ClipDescription.EXTRA_IS_REMOTE_DEVICE) == true && source == "listener") {
            // Our own write coming back.
            return
        }
        if (clip.itemCount == 0) return
        val text = clip.getItemAt(0).coerceToText(context)?.toString() ?: return
        localText(text)
    }

    /** A clip produced on the phone (read, shared, or from the tile). */
    fun localText(text: String) {
        if (text.isEmpty() || text.length > Protocol.MAX_CLIP) return
        handler.post {
            val hash = Protocol.clipHash(text)
            if (hash == currentHash) return@post
            currentHash = hash
            currentAt = System.currentTimeMillis()
            val message = JSONObject().put("text", text).put("hash", hash).put("at", currentAt)
            if (!Link.event("clipboard", message)) pending = message else pending = null
        }
    }

    override fun onEvent(topic: String, data: JSONObject?): Boolean {
        if (topic != "clipboard" || data == null) return false
        val text = data.optString("text")
        val hash = data.optString("hash").ifEmpty { Protocol.clipHash(text) }
        val at = data.optLong("at")
        if (hash == currentHash) return true
        // Both sides changed while apart: the newer clip wins.
        if (at < currentAt) return true
        currentHash = hash
        currentAt = at
        pending = null
        write(text)
        return true
    }

    private fun write(text: String) {
        suppressUntil = SystemClock.elapsedRealtime() + 1_500
        val clip = ClipData.newPlainText("phonectl", text)
        clip.description.extras = PersistableBundle().apply {
            putBoolean(ClipDescription.EXTRA_IS_REMOTE_DEVICE, true)
        }
        context.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
    }

    override fun onConnected(session: Session) {
        pending?.let { session.send(Protocol.event("clipboard", it)) }
        pending = null
    }
}
