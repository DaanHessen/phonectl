package com.daanh.phonectl

import android.app.Instrumentation
import android.app.UiAutomation
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.KeyCharacterMap
import android.view.KeyEvent

/**
 * Automatic phone → laptop clipboard with Gboard (or any keyboard) in use.
 *
 * Android 10+ lets only the focused app, the default keyboard and holders of
 * READ_CLIPBOARD_IN_BACKGROUND read the clipboard. The shell user holds that
 * permission, and an instrumentation started from a shell may borrow the
 * shell's permissions for its own app (`adoptShellPermissionIdentity`, made
 * for tests). So the laptop starts this instrumentation once over wireless
 * debugging:
 *
 *   am instrument com.daanh.phonectl/.ShellWatcher
 *
 * Android restarts phonectl's process with this class loaded in it, borrows
 * only READ_CLIPBOARD_IN_BACKGROUND, and from then on ClipSync's ordinary
 * clipboard listener is called for every copy, in the background, with no
 * polling. The instrumentation lives as long as the process (it survives the
 * adb connection ending and wireless debugging being switched off); after a
 * reboot or an app update the laptop starts it again (see [AdbWindow]).
 *
 * No accessibility service is involved (FLAG_DONT_USE_ACCESSIBILITY), so
 * banking apps that refuse to run next to one are unaffected.
 */
class ShellWatcher : Instrumentation() {
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        // Runs before App.onCreate, so the app knows from the start that it
        // was launched as the watcher and must not ask for another start.
        ShellClip.instrumented = true
        start()
    }

    override fun onStart() {
        val error = try {
            val automation = getUiAutomation(UiAutomation.FLAG_DONT_USE_ACCESSIBILITY)
            automation.adoptShellPermissionIdentity(READ_CLIPBOARD_IN_BACKGROUND)
            ShellClip.attach(automation, this)
            null
        } catch (e: Throwable) {
            e.javaClass.simpleName + (e.message?.let { ": ${it.take(80)}" } ?: "")
        }
        // App.onCreate runs on the main thread after this instrumentation is
        // created; posting there guarantees the app is fully set up.
        Handler(Looper.getMainLooper()).post { ShellClip.ready(error) }
    }
}

// Hidden in the SDK (signature permissions); the shell holds them.
private const val READ_CLIPBOARD_IN_BACKGROUND = "android.permission.READ_CLIPBOARD_IN_BACKGROUND"
private const val INSTALL_PACKAGES = "android.permission.INSTALL_PACKAGES"
private const val TETHER_PRIVILEGED = "android.permission.TETHER_PRIVILEGED"
private const val INJECT_EVENTS = "android.permission.INJECT_EVENTS"

/**
 * Shell permissions borrowed for one capability at a time, on top of the
 * clipboard permission, each with the longest time it may be held.
 */
enum class Borrow(internal val permission: String, internal val maxMs: Long) {
    /** A silent store update ([StoreUpdater]); Android checks it until the install ends. */
    INSTALL(INSTALL_PACKAGES, 2 * 60_000L),
    /** Starting or stopping the hotspot ([Hotspot]). */
    TETHER(TETHER_PRIVILEGED, 30_000L),
    /** Typing text from the laptop into the focused field ([ShellClip.type]): key events only. */
    INJECT(INJECT_EVENTS, 30_000L),
}

/**
 * State of the shell clipboard watcher, shared by the app and the
 * instrumentation. Every borrowed shell permission is listed here and nowhere
 * else (docs/security.md); the UiAutomation object never leaves this file.
 */
object ShellClip {
    private var automation: UiAutomation? = null
    private var instrumentation: Instrumentation? = null
    private val lock = Object()
    private val main = Handler(Looper.getMainLooper())
    private val borrowed = HashMap<Borrow, Runnable>()

    internal fun attach(ui: UiAutomation, watcher: Instrumentation) = synchronized(lock) {
        automation = ui
        instrumentation = watcher
    }

    /** Keys `phonectl type --key` may press. */
    val KEYS: Map<String, Int> = mapOf(
        "enter" to KeyEvent.KEYCODE_ENTER,
        "newline" to KeyEvent.KEYCODE_ENTER,
        "backspace" to KeyEvent.KEYCODE_DEL,
        "delete" to KeyEvent.KEYCODE_FORWARD_DEL,
        "tab" to KeyEvent.KEYCODE_TAB,
        "left" to KeyEvent.KEYCODE_DPAD_LEFT,
        "right" to KeyEvent.KEYCODE_DPAD_RIGHT,
        "up" to KeyEvent.KEYCODE_DPAD_UP,
        "down" to KeyEvent.KEYCODE_DPAD_DOWN,
        "escape" to KeyEvent.KEYCODE_ESCAPE,
    )

    /**
     * Types [text] into whatever has focus, then presses [key], with key events
     * only (never touches). INJECT_EVENTS is borrowed for this call alone.
     * Characters the virtual key map cannot produce (emoji, most accents) are
     * pasted instead, which replaces the phone's clipboard with [text]. Must
     * not run on the main thread (each event waits until it is delivered).
     */
    fun type(context: Context, text: String?, key: String?) {
        val watcher = synchronized(lock) { instrumentation } ?: throw IllegalStateException("the clipboard watcher is not running")
        val code = key?.let { KEYS[it] ?: throw IllegalArgumentException("unknown key $it") }
        if (!borrow(Borrow.INJECT)) throw IllegalStateException("the clipboard watcher is not running")
        try {
            if (!text.isNullOrEmpty()) {
                val events = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD).getEvents(text.toCharArray())
                if (events != null) {
                    for (event in events) watcher.sendKeySync(event)
                } else {
                    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("phonectl", text))
                    watcher.sendKeyDownUpSync(KeyEvent.KEYCODE_PASTE)
                }
            }
            if (code != null) {
                val meta = if (key == "newline") KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0
                val now = android.os.SystemClock.uptimeMillis()
                watcher.sendKeySync(KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0, meta))
                watcher.sendKeySync(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0, meta))
            }
        } finally {
            giveBack(Borrow.INJECT, "typed")
        }
    }

    /**
     * Adds [what]'s permission to the adopted set until [giveBack] or its time
     * limit. Callers check their request first (package, signer, version; or
     * the gate in [Actions]). False when the watcher is not running.
     */
    fun borrow(what: Borrow): Boolean = synchronized(lock) {
        val ui = automation ?: return false
        borrowed.remove(what)?.let(main::removeCallbacks)
        val limit = Runnable { giveBack(what, "time limit") }
        borrowed[what] = limit
        adopt(ui)
        main.postDelayed(limit, what.maxMs)
        Diag.log("${what.name.lowercase()} permission borrowed")
        true
    }

    fun giveBack(what: Borrow, reason: String): Unit = synchronized(lock) {
        val limit = borrowed.remove(what) ?: return@synchronized
        main.removeCallbacks(limit)
        automation?.let(::adopt)
        Diag.log("${what.name.lowercase()} permission returned ($reason)")
    }

    /** What is borrowed right now (for the status and tests). */
    fun borrowedNow(): List<String> = synchronized(lock) { borrowed.keys.map { it.name.lowercase() } }

    private fun adopt(ui: UiAutomation) {
        val permissions = arrayOf(READ_CLIPBOARD_IN_BACKGROUND) + borrowed.keys.map { it.permission }
        ui.adoptShellPermissionIdentity(*permissions)
    }

    /** This process was started by `am instrument` (set before App.onCreate). */
    @Volatile var instrumented = false
    /** Shell permission borrowed: background clipboard reads work. */
    @Volatile var active = false
        private set
    /** Wall-clock time the watcher became active, 0 if not running. */
    @Volatile var since = 0L
        private set
    @Volatile var error: String? = null
        private set

    fun ready(failure: String?) {
        if (failure != null) {
            error = failure
            Diag.log("shell watcher failed: $failure")
        } else {
            active = true
            since = System.currentTimeMillis()
            error = null
            Diag.log("shell watcher running: clipboard is read in the background")
        }
        ClipSync.onShellWatcher(active)
        // Started over wireless debugging that we switched on: off again.
        AdbWindow.closeIfOurs("watcher started")
        StatusMonitor.refresh()
        Link.notifyListeners()
    }
}
