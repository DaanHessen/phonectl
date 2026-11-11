package com.daanh.phonectl

import android.content.Context
import android.os.Build
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.PrintWriter
import java.io.StringWriter
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Crash reports from this app and from phonectl Keyboard, readable from the
 * laptop without ADB.
 *
 * Each report is a file in `files/crashes/`. On every connect, reports the
 * laptop has not seen yet are sent as `crash` events (the daemon logs them and
 * saves them under its data dir); `phonectl diag` also lists them.
 *
 * Reports hold stack traces and versions only, never clipboard or notification
 * content.
 */
object Crashes : Link.Feature {
    private const val TAG = "phonectl"
    private const val KEEP = 10
    private const val MAX_CHARS = 32_000
    private const val SENT = ".sent"

    private lateinit var dir: File

    /** First thing in Application.onCreate, so a crash in any later init is caught. */
    fun install(context: Context) {
        dir = File(context.filesDir, "crashes").apply { mkdirs() }
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            try {
                save("phonectl", report("phonectl ${BuildConfigVersion.NAME}", thread.name, error))
            } catch (_: Throwable) {
            }
            previous?.uncaughtException(thread, error)
        }
        Link.register(this)
    }

    fun report(app: String, thread: String, error: Throwable): String {
        val trace = StringWriter().also { error.printStackTrace(PrintWriter(it)) }
        return "App: $app\nThread: $thread\nDevice: ${Build.MANUFACTURER} ${Build.MODEL}, Android ${Build.VERSION.RELEASE} (SDK ${Build.VERSION.SDK_INT})\n\n$trace"
    }

    /** Stores a report; [source] is "phonectl" or "keyboard". */
    @Synchronized
    fun save(source: String, text: String) {
        if (!::dir.isInitialized) return
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.ROOT).format(Date())
        val safe = source.filter { it.isLetterOrDigit() }.take(20).ifEmpty { "unknown" }
        File(dir, "$stamp-$safe.txt").writeText(text.take(MAX_CHARS))
        Log.e(TAG, "crash report saved ($safe)")
        files().dropLast(KEEP).forEach { it.delete(); File(it.path + SENT).delete() }
    }

    private fun files(): List<File> =
        dir.listFiles { f -> f.name.endsWith(".txt") }.orEmpty().sortedBy { it.name }

    /** Lines for `phonectl diag`: every kept report, newest last. */
    fun dump(): List<String> {
        if (!::dir.isInitialized) return emptyList()
        val all = synchronized(this) { files() }
        if (all.isEmpty()) return emptyList()
        return listOf("=== ${all.size} crash report(s) ===") +
            all.flatMap { listOf("--- ${it.name}") + it.readText().lines() } +
            listOf("=== end of crash reports ===")
    }

    override fun onConnected(session: Session) {
        if (!::dir.isInitialized) return
        val unsent = synchronized(this) { files().filter { !File(it.path + SENT).exists() } }
        for (file in unsent) {
            val source = file.name.removeSuffix(".txt").substringAfterLast('-')
            session.send(Protocol.event("crash", JSONObject()
                .put("source", source)
                .put("name", file.name)
                .put("report", file.readText())))
            File(file.path + SENT).writeText("")
        }
        if (unsent.isNotEmpty()) Diag.log("sent ${unsent.size} crash report(s) to the laptop")
    }
}
