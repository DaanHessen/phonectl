package com.daanh.phonectl

import android.content.Context
import android.os.Build
import android.provider.Settings
import org.json.JSONObject
import java.net.Inet4Address
import java.net.NetworkInterface

/** What the phone says about itself in the handshake. */
object Hello {
    fun build(context: Context): JSONObject {
        val device = JSONObject()
            .put("model", Build.MODEL)
            .put("manufacturer", Build.MANUFACTURER)
            .put("device", Build.DEVICE)
            .put("android_version", Build.VERSION.RELEASE)
            .put("sdk", Build.VERSION.SDK_INT)
            .put("build", Build.DISPLAY)
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.let { device.put("name", it) }
        val hello = JSONObject().put("device", device).put("app_version", BuildConfigVersion.NAME)
        tailscaleAddress()?.let { hello.put("tailscale_ip", it) }
        return hello
    }

    /** The phone's 100.64.0.0/10 address, so the laptop knows where to poke. */
    fun tailscaleAddress(): String? = try {
        NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
            .flatMap { it.inetAddresses.toList() }
            .filterIsInstance<Inet4Address>()
            .map { it.hostAddress ?: "" }
            .firstOrNull { ip ->
                val parts = ip.split('.').mapNotNull { it.toIntOrNull() }
                parts.size == 4 && parts[0] == 100 && parts[1] in 64..127
            }
    } catch (_: Exception) {
        null
    }
}

object BuildConfigVersion { const val NAME = "0.2.0" }
