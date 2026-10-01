package com.daanh.phonectl

import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Everything starts here, whichever way the process comes up: the system
 * binding the notification listener (the usual case, right after boot), the
 * boot receiver, the tile, or the launcher.
 */
class App : Application() {
    override fun onCreate() {
        super.onCreate()
        StatusMonitor.start(this)
        ClipSync.start(this)
        NotifListener.init(this)
        PhoneControls.init(this)
        Updater.init(this)
        Link.start(this)
    }
}

/** Only exists so the process starts after boot and app updates. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {}
}
