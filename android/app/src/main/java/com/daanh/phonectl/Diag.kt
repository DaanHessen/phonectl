package com.daanh.phonectl

import android.util.Log
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale

/**
 * The app's own recent log lines, readable from the laptop with
 * `phonectl diag` (no ADB needed). Never put content in here: no clipboard
 * text, notification text or phone numbers. Package names and states only.
 */
object Diag {
    private val lines = ArrayDeque<String>()
    private val format = SimpleDateFormat("HH:mm:ss", Locale.ROOT)

    fun log(message: String) {
        Log.i("phonectl", message)
        synchronized(lines) {
            lines.addLast("${format.format(Date())} $message")
            while (lines.size > 200) lines.removeFirst()
        }
    }

    fun dump(): List<String> = synchronized(lines) { lines.toList() }
}
