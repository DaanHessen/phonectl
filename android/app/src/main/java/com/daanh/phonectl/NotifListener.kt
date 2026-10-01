package com.daanh.phonectl

import android.app.ActivityOptions
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Bundle
import android.os.Parcelable
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Base64
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream

/**
 * Mirrors phone notifications to the laptop.
 *
 * Being an enabled notification listener is also what keeps this process
 * alive: the system binds it right after boot and keeps it bound, so no
 * foreground service (and no permanent notification) is needed.
 *
 * What is mirrored: alerting notifications (importance default or higher)
 * that are not ongoing, not group summaries and not our own, plus ongoing
 * call notifications (so an incoming call shows up with its caller and
 * actions). Updates whose visible content did not change are dropped, so
 * progress bars and re-posts do not spam the laptop.
 */
class NotifListener : NotificationListenerService() {

    override fun onListenerConnected() {
        instance = this
        connected = true
        StatusMonitor.refresh()
        PhoneControls.startMedia()
        Link.handler.post { Link.session?.let { sendAll(it) } }
    }

    override fun onListenerDisconnected() {
        if (instance === this) instance = null
        connected = false
        StatusMonitor.refresh()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification, rankingMap: RankingMap) {
        callFromNotification(sbn, posted = true)
        val ranking = Ranking()
        val importance = if (rankingMap.getRanking(sbn.key, ranking)) ranking.importance else NotificationManager.IMPORTANCE_DEFAULT
        Link.handler.post { posted(this, sbn, importance) }
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification, rankingMap: RankingMap, reason: Int) {
        callFromNotification(sbn, posted = false)
        Link.handler.post {
            if (mirrored.remove(sbn.key) != null) Link.event("notification_removed", JSONObject().put("key", sbn.key))
        }
    }

    /**
     * Fallback call detection: an ongoing CATEGORY_CALL notification means a
     * call is ringing or active (CallStyle tells which). Its removal, with no
     * other call notification left, means the call ended.
     */
    private fun callFromNotification(sbn: StatusBarNotification, posted: Boolean) {
        val n = sbn.notification
        if (n.category != Notification.CATEGORY_CALL || !sbn.isOngoing) return
        if (posted) {
            val incoming = n.extras.getInt(Notification.EXTRA_CALL_TYPE, 0) == Notification.CallStyle.CALL_TYPE_INCOMING
            StatusMonitor.setCall(if (incoming) "ringing" else "offhook", "notification")
        } else {
            val others = try { activeNotifications } catch (_: Exception) { null }
                ?.any { it.key != sbn.key && it.notification.category == Notification.CATEGORY_CALL && it.isOngoing } ?: false
            if (!others) StatusMonitor.setCall("idle", "notification")
        }
    }

    companion object : Link.Feature {
        private const val TAG = "phonectl"
        @Volatile var instance: NotifListener? = null
        @Volatile var connected = false
        private lateinit var context: Context

        /** key → content fingerprint of what the laptop has. */
        private val mirrored = HashMap<String, String>()

        fun init(ctx: Context) {
            context = ctx.applicationContext
            Link.register(this)
        }

        private fun posted(service: NotificationListenerService, sbn: StatusBarNotification, importance: Int) {
            val json = describe(service, sbn, importance) ?: run {
                // It may have been mirrored before and turned silent/ongoing.
                if (mirrored.remove(sbn.key) != null) Link.event("notification_removed", JSONObject().put("key", sbn.key))
                return
            }
            val fingerprint = json.optString("title") + "\u0000" + json.optString("text") + "\u0000" + json.optJSONArray("actions")
            val previous = mirrored.put(sbn.key, fingerprint)
            if (previous == fingerprint) return
            json.put("update", previous != null)
            Link.event("notification", json)
        }

        private fun describe(service: NotificationListenerService, sbn: StatusBarNotification, importance: Int): JSONObject? {
            val n = sbn.notification
            if (sbn.packageName == context.packageName && n.channelId != SetupNotice.TEST_CHANNEL) return null
            if (n.flags and Notification.FLAG_GROUP_SUMMARY != 0) return null
            val isCall = n.category == Notification.CATEGORY_CALL
            if (sbn.isOngoing && !isCall) return null
            if (importance < NotificationManager.IMPORTANCE_DEFAULT && !isCall) return null
            if (n.extras.getString(Notification.EXTRA_TEMPLATE)?.endsWith("MediaStyle") == true) return null

            val extras = n.extras
            val title = (extras.getCharSequence(Notification.EXTRA_CONVERSATION_TITLE)
                ?: extras.getCharSequence(Notification.EXTRA_TITLE_BIG)
                ?: extras.getCharSequence(Notification.EXTRA_TITLE))?.toString()
            val text = body(extras)
            if (title.isNullOrBlank() && text.isNullOrBlank()) return null

            val actions = JSONArray()
            n.actions?.forEachIndexed { index, action ->
                val label = action.title?.toString() ?: return@forEachIndexed
                val reply = action.remoteInputs?.any { it.allowFreeFormInput } == true
                actions.put(JSONObject().put("index", index).put("title", label).put("reply", reply))
            }

            return JSONObject()
                .put("key", sbn.key)
                .put("package", sbn.packageName)
                .put("app", appLabel(sbn.packageName))
                .put("title", title ?: JSONObject.NULL)
                .put("text", text ?: JSONObject.NULL)
                .put("posted_at", sbn.postTime)
                .put("category", n.category ?: JSONObject.NULL)
                .put("clearable", sbn.isClearable)
                .put("can_open", n.contentIntent != null)
                .put("silent", n.flags and Notification.FLAG_ONLY_ALERT_ONCE != 0 && mirrored.containsKey(sbn.key))
                .put("actions", actions)
        }

        private fun body(extras: Bundle): String? {
            // Messaging style: the last few messages read better than the summary.
            @Suppress("DEPRECATION")
            val messages: Array<Parcelable>? = extras.getParcelableArray(Notification.EXTRA_MESSAGES)
            if (!messages.isNullOrEmpty()) {
                val lines = Notification.MessagingStyle.Message.getMessagesFromBundleArray(messages)
                    .takeLast(4)
                    .map { m -> listOfNotNull(m.senderPerson?.name?.toString(), m.text?.toString()).joinToString(": ") }
                if (lines.isNotEmpty()) return lines.joinToString("\n")
            }
            extras.getCharSequenceArray(Notification.EXTRA_TEXT_LINES)?.takeIf { it.isNotEmpty() }?.let {
                return it.joinToString("\n")
            }
            return (extras.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: extras.getCharSequence(Notification.EXTRA_TEXT))?.toString()
        }

        private fun appLabel(pkg: String): String = try {
            val pm = context.packageManager
            pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
        } catch (_: Exception) {
            pkg
        }

        private fun sendAll(session: Session) {
            val service = instance ?: return
            val items = JSONArray()
            mirrored.clear()
            val active = try { service.activeNotifications } catch (e: SecurityException) { null } ?: return
            val ranking = service.currentRanking
            for (sbn in active) {
                val r = Ranking()
                val importance = if (ranking.getRanking(sbn.key, r)) r.importance else NotificationManager.IMPORTANCE_DEFAULT
                val json = describe(service, sbn, importance) ?: continue
                mirrored[sbn.key] = json.optString("title") + "\u0000" + json.optString("text") + "\u0000" + json.optJSONArray("actions")
                items.put(json)
            }
            // The laptop reconciles: it adds what it lacks and closes what is gone.
            session.send(Protocol.event("notifications", JSONObject().put("items", items)))
        }

        override fun onConnected(session: Session) {
            Diag.log("listener ${if (instance != null) "bound" else "not bound"}; sending ${mirrored.size} known")
            sendAll(session)
        }

        override fun onCall(method: String, params: JSONObject?): Any? {
            when (method) {
                "notification_action", "notification_open", "notification_dismiss" -> {}
                "test_notification" -> { SetupNotice.test(context); return Unit }
                "diag" -> return org.json.JSONArray(Diag.dump())
                "icon" -> return icon(params?.optString("package") ?: throw IllegalArgumentException("package"))
                else -> return null
            }
            val service = instance ?: throw IllegalStateException("notification access is off")
            val key = params?.optString("key") ?: throw IllegalArgumentException("key")
            if (method == "notification_dismiss") {
                service.cancelNotification(key)
                return Unit
            }
            val sbn = service.getActiveNotifications(arrayOf(key))?.firstOrNull()
                ?: throw IllegalStateException("notification is gone")
            val intent: PendingIntent = if (method == "notification_open") {
                sbn.notification.contentIntent ?: throw IllegalStateException("nothing to open")
            } else {
                val action = sbn.notification.actions?.getOrNull(params.optInt("index", -1))
                    ?: throw IllegalArgumentException("no such action")
                action.actionIntent
            }
            // We hold the background-activity-start exemption (overlay app-op);
            // opt in to lending it, or Android 14+ blocks activity intents.
            val options = ActivityOptions.makeBasic()
                .setPendingIntentBackgroundActivityStartMode(ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED)
            intent.send(context, 0, null, null, null, null, options.toBundle())
            return Unit
        }

        private fun icon(pkg: String): JSONObject {
            val drawable = context.packageManager.getApplicationIcon(pkg)
            val size = 96
            val bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
            drawable.setBounds(0, 0, size, size)
            drawable.draw(Canvas(bitmap))
            val out = ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            Log.d(TAG, "sent icon for one package")
            return JSONObject().put("png", Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP))
        }
    }
}
