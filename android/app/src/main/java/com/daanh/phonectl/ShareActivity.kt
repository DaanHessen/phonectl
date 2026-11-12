package com.daanh.phonectl

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import org.json.JSONObject

/**
 * Share sheet target: "Send to laptop" puts shared text on the laptop
 * clipboard; a single link also shows on the laptop with an Open button.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        if (intent.action == Intent.ACTION_SEND && !text.isNullOrEmpty()) {
            ClipSync.localText(text)
            Links.webLink(text)?.let { url -> Link.event("link", JSONObject().put("url", url)) }
            val where = Link.config?.name ?: "laptop"
            Toast.makeText(this, if (Link.session != null) "Sent to $where" else "Will send to $where when connected", Toast.LENGTH_SHORT).show()
        }
        finish()
    }
}
