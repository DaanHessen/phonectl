package com.daanh.phonectl

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom

/**
 * A laptop call that needs the owner while the phone is locked: one
 * notification with Allow and Deny. The call runs only after Allow, within a
 * minute, and only once. The laptop cannot press these buttons: phonectl's own
 * notifications are never mirrored or actionable from the laptop
 * ([NotifListener]), and [ConfirmReceiver] is not exported.
 */
object Confirm {
    private const val CHANNEL = "confirm"
    private const val ID = 7
    const val TTL_MS = 60_000L

    private class Pending(val token: String, val method: String, val params: JSONObject?, val at: Long)

    private lateinit var context: Context
    private var pending: Pending? = null
    private val random = SecureRandom()

    fun init(ctx: Context) {
        context = ctx.applicationContext
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Laptop requests", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "The laptop asks before it changes something on a locked phone"
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            },
        )
    }

    /** What the owner sees, from the method and its few allowed parameters. */
    fun describe(method: String, params: JSONObject?): String = when (method) {
        "hotspot" -> if (params?.optBoolean("on") == true) "turn the hotspot on" else "turn the hotspot off"
        else -> Actions.RULES[method]?.label ?: method
    }

    /** One request at a time: a new one replaces (and so cancels) the previous one. */
    @Synchronized fun ask(method: String, params: JSONObject?) {
        val token = ByteArray(16).also(random::nextBytes).joinToString("") { "%02x".format(it) }
        pending = Pending(token, method, params?.let { JSONObject(it.toString()) }, System.currentTimeMillis())
        fun answer(code: Int, allow: Boolean) = PendingIntent.getBroadcast(
            context, code,
            Intent(context, ConfirmReceiver::class.java).putExtra(ConfirmReceiver.TOKEN, token).putExtra(ConfirmReceiver.ALLOW, allow),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val icon = Icon.createWithResource(context, R.drawable.ic_phonectl)
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_phonectl)
            .setContentTitle("Laptop asks to ${describe(method, params)}")
            .setContentText("Allow within a minute, or nothing happens")
            .setCategory(Notification.CATEGORY_STATUS)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setTimeoutAfter(TTL_MS)
            .setAutoCancel(true)
            .addAction(Notification.Action.Builder(icon, "Allow", answer(1, true)).build())
            .addAction(Notification.Action.Builder(icon, "Deny", answer(2, false)).build())
            .build()
        try {
            context.getSystemService(NotificationManager::class.java).notify(ID, notification)
        } catch (_: SecurityException) {
            pending = null
            Actions.record(method, "could not ask: notifications are off")
        }
    }

    @Synchronized fun answer(token: String, allow: Boolean, now: Long = System.currentTimeMillis()) {
        val p = pending ?: return
        if (!MessageDigest.isEqual(p.token.toByteArray(), token.toByteArray())) return
        pending = null
        context.getSystemService(NotificationManager::class.java).cancel(ID)
        val result = JSONObject().put("method", p.method)
        when {
            now - p.at > TTL_MS -> {
                Actions.record(p.method, "not run: the confirmation expired")
                Link.event("action_result", result.put("error", "expired"))
            }
            !allow -> {
                Actions.record(p.method, "denied on the phone")
                Link.event("action_result", result.put("error", "denied on the phone"))
            }
            else -> {
                Actions.record(p.method, "allowed on the phone")
                Link.handler.post {
                    Link.invoke(p.method, p.params) { reply ->
                        reply.optString("error").takeIf { it.isNotEmpty() }?.let { result.put("error", it) }
                        Link.event("action_result", result)
                    }
                }
            }
        }
    }
}

/** Allow / Deny on a [Confirm] notification. Not exported: only our own PendingIntents reach it. */
class ConfirmReceiver : BroadcastReceiver() {
    companion object {
        const val TOKEN = "token"
        const val ALLOW = "allow"
    }

    override fun onReceive(context: Context, intent: Intent) {
        Confirm.answer(intent.getStringExtra(TOKEN) ?: return, intent.getBooleanExtra(ALLOW, false))
    }
}
