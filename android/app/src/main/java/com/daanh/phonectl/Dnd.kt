package com.daanh.phonectl

import android.app.NotificationManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.service.notification.NotificationListenerService
import org.json.JSONObject

/**
 * Do Not Disturb, synced with the laptop: its state is part of the status
 * (the laptop follows changes), and the laptop's changes come in as
 * `dnd {on}`. "On" is Android's priority mode, so the owner's own DND
 * exceptions (starred contacts, alarms) keep working.
 */
object Dnd : Link.Feature {
    private lateinit var context: Context

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        Link.handler.post {
            context.registerReceiver(object : BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) = StatusMonitor.refresh()
            }, IntentFilter(NotificationManager.ACTION_INTERRUPTION_FILTER_CHANGED), null, Link.handler)
        }
    }

    fun on(): Boolean = when (context.getSystemService(NotificationManager::class.java).currentInterruptionFilter) {
        NotificationManager.INTERRUPTION_FILTER_ALL, NotificationManager.INTERRUPTION_FILTER_UNKNOWN -> false
        else -> true
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "dnd") return null
        if (params == null || !params.has("on")) throw IllegalArgumentException("on")
        val wanted = params.get("on") as? Boolean ?: throw IllegalArgumentException("on must be true or false")
        if (wanted == on()) return Unit
        val nm = context.getSystemService(NotificationManager::class.java)
        val listener = NotifListener.instance
        when {
            nm.isNotificationPolicyAccessGranted ->
                nm.setInterruptionFilter(if (wanted) NotificationManager.INTERRUPTION_FILTER_PRIORITY else NotificationManager.INTERRUPTION_FILTER_ALL)
            listener != null ->
                listener.requestInterruptionFilter(if (wanted) NotificationListenerService.INTERRUPTION_FILTER_PRIORITY else NotificationListenerService.INTERRUPTION_FILTER_ALL)
            else -> throw IllegalStateException("needs notification access or Do Not Disturb access")
        }
        return Unit
    }
}
