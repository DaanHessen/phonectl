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
    const val ACTION_CRASH = "com.daanh.phonectl.action.CRASH"

    private lateinit var context: Context
    const val MAX_TYPED = 4000

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

    private val typist = java.util.concurrent.Executors.newSingleThreadExecutor { r -> Thread(r, "typist") }

    /**
     * Text typed into the focused field on the phone, sent from the laptop:
     * through phonectl Keyboard when it is the keyboard, otherwise as key
     * events through the shell identity (any keyboard, see [ShellClip.type]).
     */
    fun type(text: String?, key: String?): Any {
        if (isDefault()) {
            val intent = Intent(ACTION_TYPE).setPackage(PACKAGE)
            if (!text.isNullOrEmpty()) intent.putExtra("text", text)
            if (!key.isNullOrEmpty()) intent.putExtra("key", key)
            context.sendBroadcast(intent, PERMISSION)
            return Unit
        }
        if (!ShellClip.active) throw IllegalStateException("typing needs the clipboard watcher running (phonectl setup-clipboard)")
        if (key != null && key !in ShellClip.KEYS) throw IllegalArgumentException("unknown key $key")
        return java.util.concurrent.CompletableFuture.supplyAsync({ ShellClip.type(context, text, key); Unit }, typist)
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "type_text") return null
        // optString turns JSON null into "null": check isNull first.
        val text = params?.takeUnless { it.isNull("text") }?.optString("text")?.takeIf { it.isNotEmpty() }
        val key = params?.takeUnless { it.isNull("key") }?.optString("key")?.takeIf { it.isNotEmpty() }
        if (text == null && key == null) throw IllegalArgumentException("text or key")
        if (text != null && text.length > MAX_TYPED) throw IllegalArgumentException("text too long (at most $MAX_TYPED characters)")
        if (text != null && text.any { it.isISOControl() && it != '\n' && it != '\t' }) throw IllegalArgumentException("control characters")
        return type(text, key)
    }
}

/** Receives clips from phonectl Keyboard (protected by [Keyboard.PERMISSION] in the manifest). */
class KeyboardBridge : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Keyboard.ACTION_CLIP -> ClipSync.localText(intent.getStringExtra("text") ?: return)
            // phonectl Keyboard crashed: keep its report for the laptop.
            Keyboard.ACTION_CRASH -> Crashes.save("keyboard", intent.getStringExtra("report") ?: return)
        }
    }
}
