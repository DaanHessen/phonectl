package com.daanh.phonectl

import android.app.Instrumentation
import android.app.UiAutomation
import android.os.Bundle
import android.os.Handler
import android.os.Looper

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
            ShellClip.attach(automation)
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

/**
 * State of the shell clipboard watcher, shared by the app and the
 * instrumentation. Every borrowed shell permission is listed here and nowhere
 * else (docs/security.md); the UiAutomation object never leaves this file.
 */
object ShellClip {
    private var automation: UiAutomation? = null
    private val lock = Object()

    internal fun attach(ui: UiAutomation) = synchronized(lock) { automation = ui }

    private val main = Handler(Looper.getMainLooper())
    private val giveBack = Runnable { returnInstallPermission("time limit") }

    /**
     * Borrows INSTALL_PACKAGES as well, until [returnInstallPermission] (the
     * install finished) or two minutes at most. Android checks it while the
     * install runs, after `commit()` returns, so it cannot be scoped to one
     * call. Only [StoreUpdater] calls this, after checking the APK's package,
     * version and signing certificate. False when the watcher is not running.
     */
    fun borrowInstallPermission(): Boolean = synchronized(lock) {
        val ui = automation ?: return false
        ui.adoptShellPermissionIdentity(READ_CLIPBOARD_IN_BACKGROUND, INSTALL_PACKAGES)
        main.removeCallbacks(giveBack)
        main.postDelayed(giveBack, 2 * 60_000L)
        Diag.log("install permission borrowed")
        true
    }

    fun returnInstallPermission(reason: String): Unit = synchronized(lock) {
        main.removeCallbacks(giveBack)
        val ui = automation ?: return@synchronized
        ui.adoptShellPermissionIdentity(READ_CLIPBOARD_IN_BACKGROUND)
        Diag.log("install permission returned ($reason)")
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
