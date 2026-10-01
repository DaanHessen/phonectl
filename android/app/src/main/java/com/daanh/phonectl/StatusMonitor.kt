package com.daanh.phonectl

import android.Manifest
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.SystemClock
import android.telephony.ServiceState
import android.telephony.SignalStrength
import android.telephony.TelephonyCallback
import android.telephony.TelephonyDisplayInfo
import android.telephony.TelephonyManager
import org.json.JSONObject

/**
 * Phone status for the Waybar module: battery, network, cellular type and
 * signal, call state.
 *
 * Every input is an event source (sticky battery broadcast, connectivity and
 * telephony callbacks); nothing is polled. Changes are coalesced for a second
 * and only sent when the resulting status differs from what the laptop last
 * got. Signal-bar flapping is rate limited to one update a minute.
 */
object StatusMonitor : Link.Feature {
    private lateinit var context: Context
    private val handler get() = Link.handler

    private var battery = JSONObject()
    private var networkType = "none"        // wifi | cellular | ethernet | none
    private var wifiLevel: Int? = null       // 0-4
    private var cellularType: String? = null // 5G, LTE, 3G, …
    private var cellularLevel: Int? = null   // 0-4
    private var operator: String? = null
    private var callState = "idle"

    private var lastSent: String? = null
    private var lastSignalOnlySend = 0L
    private val flush = Runnable { send(force = false) }

    fun start(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        handler.post {
            // Battery: BATTERY_CHANGED is sticky and only delivered while awake.
            context.registerReceiver(object : BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) = onBattery(intent)
            }, IntentFilter(Intent.ACTION_BATTERY_CHANGED), null, handler)?.let(::onBattery)

            val cm = context.getSystemService(ConnectivityManager::class.java)
            cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = onNetwork(caps)
                override fun onLost(network: Network) {
                    networkType = "none"
                    schedule(signalOnly = false)
                }
            }, handler)

            // The default network is the Tailscale VPN, whose capabilities
            // carry no Wi-Fi signal; watch the Wi-Fi network itself for that.
            cm.registerNetworkCallback(
                NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build(),
                object : ConnectivityManager.NetworkCallback() {
                    override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                        val level = if (caps.signalStrength != NetworkCapabilities.SIGNAL_STRENGTH_UNSPECIFIED) {
                            context.getSystemService(WifiManager::class.java).calculateSignalLevel(caps.signalStrength).coerceAtMost(4)
                        } else null
                        if (level != wifiLevel) {
                            wifiLevel = level
                            schedule(signalOnly = true)
                        }
                    }
                    override fun onLost(network: Network) {
                        wifiLevel = null
                        schedule(signalOnly = false)
                    }
                }, handler)

            // Call state, device-wide (every SIM): the telephony callback below
            // is per subscription and has been seen to stay silent on this
            // phone. Either source may fire first; setCall() dedupes.
            context.registerReceiver(object : BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) {
                    when (intent.getStringExtra(TelephonyManager.EXTRA_STATE)) {
                        TelephonyManager.EXTRA_STATE_RINGING -> setCall("ringing", "broadcast")
                        TelephonyManager.EXTRA_STATE_OFFHOOK -> setCall("offhook", "broadcast")
                        TelephonyManager.EXTRA_STATE_IDLE -> setCall("idle", "broadcast")
                    }
                }
            }, IntentFilter(TelephonyManager.ACTION_PHONE_STATE_CHANGED), null, handler)

            registerTelephony()
        }
    }

    /**
     * Single entry for call state from any source (telephony callback,
     * phone-state broadcast, or the dialer's call notification).
     */
    fun setCall(next: String, source: String) {
        handler.post {
            if (next == callState) return@post
            Diag.log("call state $next (from $source)")
            callState = next
            // Calls are time-critical (the laptop pauses media): send a
            // dedicated event right away, not coalesced.
            Link.event("call", JSONObject().put("state", next).put("at", System.currentTimeMillis()))
            schedule(signalOnly = false)
        }
    }

    /** Something outside the monitored sources changed (e.g. a permission). */
    fun refresh() {
        if (::context.isInitialized) handler.post { schedule(signalOnly = false) }
    }

    /** Called again after READ_PHONE_STATE is granted from the app UI. */
    fun registerTelephony() {
        if (!::context.isInitialized) return
        handler.post {
            if (telephonyRegistered) return@post
            val tm = context.getSystemService(TelephonyManager::class.java) ?: return@post
            val callback = PhoneCallback()
            try {
                tm.registerTelephonyCallback(context.mainExecutor, callback)
                telephonyRegistered = true
                Diag.log("telephony callback registered")
            } catch (_: SecurityException) {
                Diag.log("telephony callback: no READ_PHONE_STATE")
                // No READ_PHONE_STATE yet: register the permission-free parts.
                tm.registerTelephonyCallback(context.mainExecutor, SignalOnlyCallback())
            }
            operator = operatorName(null)
        }
    }
    private var telephonyRegistered = false

    private fun onBattery(intent: Intent) {
        val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
        val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, 0)
        val plugged = intent.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0)
        val next = JSONObject()
            .put("level", if (level >= 0 && scale > 0) level * 100 / scale else JSONObject.NULL)
            .put("charging", status == BatteryManager.BATTERY_STATUS_CHARGING)
            .put("full", status == BatteryManager.BATTERY_STATUS_FULL)
            .put("plugged", when (plugged) {
                BatteryManager.BATTERY_PLUGGED_AC -> "ac"
                BatteryManager.BATTERY_PLUGGED_USB -> "usb"
                BatteryManager.BATTERY_PLUGGED_WIRELESS -> "wireless"
                0 -> JSONObject.NULL
                else -> "other"
            })
        if (next.toString() != battery.toString()) {
            battery = next
            schedule(signalOnly = false)
        }
    }

    private fun onNetwork(caps: NetworkCapabilities) {
        // With Tailscale up the default network is the VPN, which carries its
        // underlying transport, so these checks still see Wi-Fi vs cellular.
        val type = when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_BLUETOOTH) -> "bluetooth"
            else -> "other"
        }
        if (type != networkType) {
            networkType = type
            schedule(signalOnly = false)
        }
    }

    private open class SignalOnlyCallback : TelephonyCallback(), TelephonyCallback.SignalStrengthsListener {
        override fun onSignalStrengthsChanged(signal: SignalStrength) {
            handler.post {
                if (signal.level != cellularLevel) {
                    cellularLevel = signal.level
                    schedule(signalOnly = true)
                }
            }
        }
    }

    private class PhoneCallback : SignalOnlyCallback(),
        TelephonyCallback.DisplayInfoListener,
        TelephonyCallback.CallStateListener,
        TelephonyCallback.ServiceStateListener {

        override fun onDisplayInfoChanged(info: TelephonyDisplayInfo) {
            handler.post {
                val type = displayType(info)
                if (type != cellularType) {
                    cellularType = type
                    schedule(signalOnly = false)
                }
            }
        }

        override fun onCallStateChanged(state: Int) {
            setCall(when (state) {
                TelephonyManager.CALL_STATE_RINGING -> "ringing"
                TelephonyManager.CALL_STATE_OFFHOOK -> "offhook"
                else -> "idle"
            }, "callback")
        }

        override fun onServiceStateChanged(state: ServiceState) {
            handler.post {
                val name = operatorName(state)
                if (name != operator) {
                    operator = name
                    schedule(signalOnly = false)
                }
            }
        }
    }

    /** ServiceState names are location-gated; TelephonyManager's are not. */
    private fun operatorName(state: ServiceState?): String? {
        val tm = context.getSystemService(TelephonyManager::class.java)
        return state?.operatorAlphaShort?.takeIf { it.isNotBlank() }
            ?: tm?.networkOperatorName?.takeIf { it.isNotBlank() }
            ?: tm?.simOperatorName?.takeIf { it.isNotBlank() }
    }

    private fun displayType(info: TelephonyDisplayInfo): String? = when (info.overrideNetworkType) {
        TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_NSA,
        TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_NR_ADVANCED -> "5G"
        TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_ADVANCED_PRO -> "LTE+"
        TelephonyDisplayInfo.OVERRIDE_NETWORK_TYPE_LTE_CA -> "LTE+"
        else -> when (info.networkType) {
            TelephonyManager.NETWORK_TYPE_NR -> "5G"
            TelephonyManager.NETWORK_TYPE_LTE -> "LTE"
            TelephonyManager.NETWORK_TYPE_HSPAP, TelephonyManager.NETWORK_TYPE_HSPA,
            TelephonyManager.NETWORK_TYPE_HSDPA, TelephonyManager.NETWORK_TYPE_HSUPA,
            TelephonyManager.NETWORK_TYPE_UMTS -> "3G"
            TelephonyManager.NETWORK_TYPE_EDGE, TelephonyManager.NETWORK_TYPE_GPRS -> "2G"
            TelephonyManager.NETWORK_TYPE_UNKNOWN -> null
            else -> "other"
        }
    }

    fun snapshot(): JSONObject = JSONObject()
        .put("battery", battery)
        .put("network", JSONObject()
            .put("type", networkType)
            .put("wifi_level", wifiLevel ?: JSONObject.NULL)
            .put("cellular", cellularType ?: JSONObject.NULL)
            .put("cellular_level", cellularLevel ?: JSONObject.NULL)
            .put("operator", operator ?: JSONObject.NULL))
        .put("call", callState)
        .put("media", PhoneControls.media ?: JSONObject.NULL)
        .put("ringer", PhoneControls.ringerMode())
        .put("ringing", PhoneControls.ringing)
        .put("permissions", JSONObject()
            .put("phone_state", granted(Manifest.permission.READ_PHONE_STATE))
            .put("bluetooth", granted(Manifest.permission.BLUETOOTH_CONNECT))
            .put("notifications", NotifListener.connected)
            .put("clipboard_auto", ClipSync.autoAvailable()))

    private fun granted(p: String) = context.checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    private fun schedule(signalOnly: Boolean) {
        if (signalOnly) {
            // Signal bars move constantly; one update a minute is plenty.
            val wait = 60_000 - (SystemClock.elapsedRealtime() - lastSignalOnlySend)
            if (!handler.hasCallbacks(flush)) handler.postDelayed(flush, wait.coerceIn(1_000, 60_000))
        } else {
            handler.removeCallbacks(flush)
            handler.postDelayed(flush, 1_000)
        }
    }

    private fun send(force: Boolean) {
        val status = snapshot()
        val text = status.toString()
        if (!force && text == lastSent) return
        if (Link.event("status", status.put("at", System.currentTimeMillis()))) {
            lastSent = text
            lastSignalOnlySend = SystemClock.elapsedRealtime()
        }
    }

    override fun onConnected(session: Session) {
        handler.removeCallbacks(flush)
        send(force = true)
    }

    override fun onDisconnected() {
        lastSent = null
    }
}
