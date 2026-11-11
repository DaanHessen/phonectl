package com.daanh.phonectl

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.provider.Settings
import org.json.JSONObject

/**
 * Bridge to phonectl Keyboard (keyboard/ in this repo: HeliBoard plus a small
 * patch), a separate app signed with the same key.
 *
 * Why a keyboard: since Android 10 only the focused app and the *default
 * input method* may read the clipboard or hear about changes to it
 * (ClipboardService.clipboardAccessAllowed). The default IME is also bound by
 * the system for as long as it is selected, so it is alive whenever the phone
 * is. When phonectl Keyboard is the selected keyboard, every copy on the phone
 * reaches us here with no ADB grant, no log access and no tap after a reboot.
 *
 * Both directions are explicit broadcasts guarded by a signature permission
 * that both apps declare, so only apps signed with our key can send or
 * receive them.
 */
object Keyboard : Link.Feature {
    const val PACKAGE = "com.daanh.phonectl.keyboard"
    const val PERMISSION = "com.daanh.phonectl.permission.BRIDGE"
    const val ACTION_CLIP = "com.daanh.phonectl.action.CLIP"
    const val ACTION_TYPE = "com.daanh.phonectl.action.TYPE"

    private lateinit var context: Context

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
    }

    /** True when phonectl Keyboard is the keyboard Android is using now. */
    fun isDefault(ctx: Context = context): Boolean {
        val id = Settings.Secure.getString(ctx.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD) ?: return false
        return ComponentName.unflattenFromString(id)?.packageName == PACKAGE
    }

    fun installed(ctx: Context = context): Boolean = try {
        ctx.packageManager.getPackageInfo(PACKAGE, 0); true
    } catch (_: Exception) {
        false
    }

    /** Text typed into the focused field on the phone, sent from the laptop. */
    fun type(text: String?, key: String?) {
        if (!isDefault()) throw IllegalStateException("phonectl Keyboard is not the active keyboard on the phone")
        val intent = Intent(ACTION_TYPE).setPackage(PACKAGE)
        if (!text.isNullOrEmpty()) intent.putExtra("text", text)
        if (!key.isNullOrEmpty()) intent.putExtra("key", key)
        context.sendBroadcast(intent, PERMISSION)
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "type_text") return null
        // optString turns JSON null into "null": check isNull first.
        val text = params?.takeUnless { it.isNull("text") }?.optString("text")?.takeIf { it.isNotEmpty() }
        val key = params?.takeUnless { it.isNull("key") }?.optString("key")?.takeIf { it.isNotEmpty() }
        if (text == null && key == null) throw IllegalArgumentException("text or key")
        if (text != null && text.length > Protocol.MAX_CLIP) throw IllegalArgumentException("text too long")
        type(text, key)
        return Unit
    }
}

/** Receives clips from phonectl Keyboard (protected by [Keyboard.PERMISSION] in the manifest). */
class KeyboardBridge : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Keyboard.ACTION_CLIP) return
        val text = intent.getStringExtra("text") ?: return
        ClipSync.localText(text)
    }
}
