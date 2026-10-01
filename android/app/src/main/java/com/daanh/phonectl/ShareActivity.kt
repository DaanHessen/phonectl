package com.daanh.phonectl

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/** Share sheet target: "phonectl" puts shared text on the laptop clipboard. */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        if (intent.action == Intent.ACTION_SEND && !text.isNullOrEmpty()) {
            ClipSync.localText(text)
            val where = Link.config?.name ?: "laptop"
            Toast.makeText(this, if (Link.session != null) "Sent to $where" else "Will send to $where when connected", Toast.LENGTH_SHORT).show()
        }
        finish()
    }
}
