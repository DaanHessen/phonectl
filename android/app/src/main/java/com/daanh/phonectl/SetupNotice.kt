package com.daanh.phonectl

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent

/** The one quiet notification phonectl may post: "tap to re-arm clipboard". */
object SetupNotice {
    private const val CHANNEL = "setup"
    private const val ID = 1

    fun clipboardNeedsTap(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Setup", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(
            context, 0,
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_phonectl)
            .setContentTitle("Clipboard sync to laptop is paused")
            .setContentText("Tap, then allow log access, to copy to the laptop automatically again")
            .setContentIntent(open)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .build()
        try { nm.notify(ID, notification) } catch (_: SecurityException) {}
    }

    /** `phonectl test-notification`: a known notification to test mirroring with. */
    fun test(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(TEST_CHANNEL, "Test", NotificationManager.IMPORTANCE_DEFAULT))
        val open = PendingIntent.getActivity(
            context, 1, Intent(context, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(context, TEST_CHANNEL)
            .setSmallIcon(R.drawable.ic_phonectl)
            .setContentTitle("phonectl test")
            .setContentText("Swipe me away on the phone, or dismiss me on the laptop")
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Open app", open).build())
            .setAutoCancel(true)
            .build()
        nm.notify(TEST_ID, n)
    }

    const val TEST_CHANNEL = "test"
    private const val TEST_ID = 2

    fun clear(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(ID)
    }
}
