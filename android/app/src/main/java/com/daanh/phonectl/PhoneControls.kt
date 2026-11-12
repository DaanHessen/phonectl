package com.daanh.phonectl

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.Ringtone
import android.media.RingtoneManager
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.app.NotificationManager
import org.json.JSONObject

/**
 * Things the laptop's phone menu can do and show:
 *  - now playing on the phone, with play/pause/next/previous
 *  - ringer mode (normal / vibrate / silent)
 *  - "find my phone": ring at full alarm volume, even on silent, until a
 *    touch on the phone, `phonectl ring --stop` or 60 s
 *
 * Media sessions are readable because we are an enabled notification
 * listener; everything is callback-driven and only changes are sent (as part
 * of the status, via [StatusMonitor]).
 */
object PhoneControls : Link.Feature {
    private lateinit var context: Context
    private val handler get() = Link.handler

    @Volatile var media: JSONObject? = null
        private set
    @Volatile var ringing = false
        private set
    private var ringtone: Ringtone? = null
    private var controller: MediaController? = null
    private var mediaStarted = false

    private val ringTimeout = Runnable { ring(false) }

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        handler.post {
            context.registerReceiver(object : BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) = StatusMonitor.refresh()
            }, IntentFilter(AudioManager.RINGER_MODE_CHANGED_ACTION), null, handler)
        }
    }

    fun ringerMode(): String = when (context.getSystemService(AudioManager::class.java).ringerMode) {
        AudioManager.RINGER_MODE_SILENT -> "silent"
        AudioManager.RINGER_MODE_VIBRATE -> "vibrate"
        else -> "normal"
    }

    /** Called once notification access is live (session listing needs it). */
    fun startMedia() {
        handler.post {
            if (mediaStarted) return@post
            val msm = context.getSystemService(MediaSessionManager::class.java)
            val component = ComponentName(context, NotifListener::class.java)
            try {
                msm.addOnActiveSessionsChangedListener({ sessions -> follow(sessions?.firstOrNull()) }, component, handler)
                follow(msm.getActiveSessions(component).firstOrNull())
                mediaStarted = true
            } catch (_: SecurityException) {
            }
        }
    }

    private val callback = object : MediaController.Callback() {
        override fun onPlaybackStateChanged(state: PlaybackState?) = update()
        override fun onMetadataChanged(metadata: MediaMetadata?) = update()
        override fun onSessionDestroyed() = follow(null)
    }

    private fun follow(next: MediaController?) {
        if (next?.sessionToken == controller?.sessionToken) return
        controller?.unregisterCallback(callback)
        controller = next
        next?.registerCallback(callback, handler)
        update()
    }

    private fun update() {
        val c = controller
        val meta = c?.metadata
        val title = meta?.getString(MediaMetadata.METADATA_KEY_TITLE)
        val next = if (c == null || title.isNullOrBlank()) null else JSONObject()
            .put("app", appLabel(c.packageName))
            .put("title", title)
            .put("artist", meta.getString(MediaMetadata.METADATA_KEY_ARTIST) ?: JSONObject.NULL)
            .put("playing", c.playbackState?.state == PlaybackState.STATE_PLAYING)
        if (next?.toString() != media?.toString()) {
            media = next
            StatusMonitor.refresh()
        }
    }

    private fun appLabel(pkg: String): String = try {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    } catch (_: Exception) {
        pkg
    }

    private var savedAlarmVolume: Int? = null

    /** Stops a ring from the phone itself (touch or key on [RingActivity]). */
    fun stopRing(reason: String) {
        handler.post {
            if (!ringing) return@post
            ring(false)
            Actions.note("ring", "ring the phone", "stopped: $reason")
        }
    }

    private fun ring(on: Boolean) {
        handler.removeCallbacks(ringTimeout)
        ringtone?.stop()
        ringtone = null
        val am = context.getSystemService(AudioManager::class.java)
        savedAlarmVolume?.let { try { am.setStreamVolume(AudioManager.STREAM_ALARM, it, 0) } catch (_: Exception) {} }
        savedAlarmVolume = null
        val wasRinging = ringing
        ringing = false
        if (on) {
            val uri = RingtoneManager.getActualDefaultRingtoneUri(context, RingtoneManager.TYPE_RINGTONE)
                ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM)
            val tone = RingtoneManager.getRingtone(context, uri) ?: throw IllegalStateException("no ringtone")
            // Alarm usage plays even in vibrate/silent mode; full alarm volume
            // for the ring, put back afterwards.
            tone.audioAttributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build()
            try {
                savedAlarmVolume = am.getStreamVolume(AudioManager.STREAM_ALARM)
                am.setStreamVolume(AudioManager.STREAM_ALARM, am.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0)
            } catch (_: Exception) {
                savedAlarmVolume = null
            }
            tone.isLooping = true
            tone.play()
            ringtone = tone
            ringing = true
            handler.postDelayed(ringTimeout, 60_000)
            showStopScreen()
        } else if (wasRinging) {
            context.getSystemService(android.app.NotificationManager::class.java).cancel(RING_NOTIFICATION)
        }
        StatusMonitor.refresh()
    }

    private const val RING_CHANNEL = "ring"
    private const val RING_NOTIFICATION = 8

    /** A full-screen "touch to stop" screen; the notification's Stop is the fallback. */
    private fun showStopScreen() {
        val nm = context.getSystemService(android.app.NotificationManager::class.java)
        nm.createNotificationChannel(android.app.NotificationChannel(RING_CHANNEL, "Find my phone", android.app.NotificationManager.IMPORTANCE_HIGH).apply {
            setSound(null, null)
        })
        val screen = android.content.Intent(context, RingActivity::class.java)
            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_NO_USER_ACTION)
        val full = android.app.PendingIntent.getActivity(context, 3, screen, android.app.PendingIntent.FLAG_IMMUTABLE)
        val stop = android.app.PendingIntent.getBroadcast(
            context, 4,
            android.content.Intent(context, RingStopReceiver::class.java),
            android.app.PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = android.app.Notification.Builder(context, RING_CHANNEL)
            .setSmallIcon(R.drawable.ic_phonectl)
            .setContentTitle("Ringing from the laptop")
            .setContentText("Touch to stop")
            .setCategory(android.app.Notification.CATEGORY_ALARM)
            .setVisibility(android.app.Notification.VISIBILITY_PUBLIC)
            .setFullScreenIntent(full, true)
            .setContentIntent(full)
            .setDeleteIntent(stop)
            .addAction(android.app.Notification.Action.Builder(null, "Stop", stop).build())
            .build()
        try { nm.notify(RING_NOTIFICATION, notification) } catch (_: SecurityException) {}
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        when (method) {
            "ring" -> ring(params?.optBoolean("on", true) ?: true)
            "ringer" -> {
                val am = context.getSystemService(AudioManager::class.java)
                val mode = when (params?.optString("mode")) {
                    "normal" -> AudioManager.RINGER_MODE_NORMAL
                    "vibrate" -> AudioManager.RINGER_MODE_VIBRATE
                    "silent" -> {
                        if (!context.getSystemService(NotificationManager::class.java).isNotificationPolicyAccessGranted) {
                            throw IllegalStateException("silent needs Do Not Disturb access (phonectl setup grants it)")
                        }
                        AudioManager.RINGER_MODE_SILENT
                    }
                    else -> throw IllegalArgumentException("mode")
                }
                am.ringerMode = mode
            }
            "media_action" -> {
                val c = controller ?: throw IllegalStateException("nothing is playing")
                val t = c.transportControls
                when (params?.optString("action")) {
                    "play_pause" -> if (c.playbackState?.state == PlaybackState.STATE_PLAYING) t.pause() else t.play()
                    "play" -> t.play()
                    "pause" -> t.pause()
                    "next" -> t.skipToNext()
                    "previous" -> t.skipToPrevious()
                    else -> throw IllegalArgumentException("action")
                }
            }
            else -> return null
        }
        return Unit
    }
}

/** "Stop" on the ring notification (or swiping it away). Not exported. */
class RingStopReceiver : android.content.BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) = PhoneControls.stopRing("stopped on the phone")
}
