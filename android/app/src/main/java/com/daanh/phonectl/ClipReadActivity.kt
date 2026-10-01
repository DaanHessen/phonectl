package com.daanh.phonectl

import android.app.Activity
import android.os.Bundle

/**
 * Invisible, focusable for one frame: Android only lets the focused app read
 * the clipboard. Started by [ClipSync] when a change is detected and by the
 * Quick Settings tile.
 */
class ClipReadActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        overrideActivityTransition(OVERRIDE_TRANSITION_OPEN, 0, 0)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus) return
        ClipSync.readNow(intent.getStringExtra("source") ?: "change")
        finish()
        overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
    }
}
