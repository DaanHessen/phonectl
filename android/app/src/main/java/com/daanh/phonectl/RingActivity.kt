package com.daanh.phonectl

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.widget.TextView

/**
 * Shown over the lock screen while `phonectl ring` rings: any touch or any
 * key (volume, power) stops the ring. Not exported; only [PhoneControls]
 * starts it.
 */
class RingActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setShowWhenLocked(true)
        setTurnScreenOn(true)
        setContentView(TextView(this).apply {
            text = "Ringing from the laptop\n\nTouch anywhere to stop"
            gravity = Gravity.CENTER
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 28f)
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.BLACK)
        })
        if (!PhoneControls.ringing) finish()
    }

    private fun stop() {
        PhoneControls.stopRing("touched on the phone")
        finish()
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.action == MotionEvent.ACTION_DOWN) stop()
        return true
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        stop()
        return true
    }
}
