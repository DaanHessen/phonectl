package com.daanh.phonectl

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.util.Base64
import android.util.Log
import org.json.JSONObject

/**
 * `phonectl update` on the laptop: the APK arrives over the link and is
 * installed with PackageInstaller, so updating never needs ADB (which some
 * payment apps refuse to run alongside).
 *
 * The first update asks for confirmation (and "install unknown apps" for
 * phonectl); after that phonectl is its own installer of record and updates
 * install without a prompt (USER_ACTION_NOT_REQUIRED, Android 12+).
 */
object Updater : Link.Feature {
    private lateinit var context: Context

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "install_update") return null
        val apk = Base64.decode(params?.optString("apk") ?: throw IllegalArgumentException("apk"), Base64.DEFAULT)
        val installer = context.packageManager.packageInstaller
        val sessionParams = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val id = installer.createSession(sessionParams)
        installer.openSession(id).use { session ->
            session.openWrite("phonectl.apk", 0, apk.size.toLong()).use { out ->
                out.write(apk)
                session.fsync(out)
            }
            val intent = Intent(context, UpdateReceiver::class.java)
            val pending = PendingIntent.getBroadcast(context, id, intent, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            session.commit(pending.intentSender)
        }
        Log.i("phonectl", "update staged (${apk.size} bytes)")
        return JSONObject().put("staged", apk.size)
    }
}

class UpdateReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            @Suppress("DEPRECATION")
            val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
            confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try { context.startActivity(confirm) } catch (e: Exception) { Log.w("phonectl", "cannot show update prompt") }
            Link.event("update", JSONObject().put("status", "needs confirmation on the phone"))
        } else {
            val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: ""
            Log.i("phonectl", "update finished: $status $message")
            Link.event("update", JSONObject().put("status", if (status == PackageInstaller.STATUS_SUCCESS) "installed" else "failed: $message"))
        }
    }
}
