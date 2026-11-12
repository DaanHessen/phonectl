package com.daanh.phonectl

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import org.json.JSONObject

/**
 * Links both ways. Laptop → phone (`phonectl share URL`): a notification
 * with the link; it opens only when the owner taps it. Phone → laptop:
 * sharing a single link to "Send to laptop" also sends it as a `link` event,
 * which the laptop shows with an Open button (never opened by itself).
 * Only http and https links, at most [MAX_URL] characters.
 */
object Links : Link.Feature {
    const val MAX_URL = 2048
    private const val CHANNEL = "links"
    private lateinit var context: Context
    private var next = 100

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Links from the laptop", NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** The URL if [text] is exactly one http(s) link, else null. */
    fun webLink(text: String): String? {
        val t = text.trim()
        if (t.isEmpty() || t.length > MAX_URL || t.any { it.isWhitespace() || it.isISOControl() }) return null
        val scheme = t.substringBefore(':', "").lowercase()
        if (scheme != "http" && scheme != "https") return null
        if (!t.substring(scheme.length).startsWith("://") || t.length <= scheme.length + 3) return null
        return t
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "open_link") return null
        val url = params?.optString("url")?.let(::webLink) ?: throw IllegalArgumentException("only one http or https link")
        val view = Intent(Intent.ACTION_VIEW, Uri.parse(url))
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val id = synchronized(this) { next++.also { if (next > 199) next = 100 } }
        val open = PendingIntent.getActivity(context, id, view, PendingIntent.FLAG_IMMUTABLE)
        val host = Uri.parse(url).host ?: url
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_phonectl)
            .setContentTitle("Link from ${Link.config?.name ?: "the laptop"}: $host")
            .setContentText(url)
            .setStyle(Notification.BigTextStyle().bigText(url))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(id, notification)
        return Unit
    }
}
