package com.daanh.phonectl

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast

/**
 * Status and one-time setup. Nothing here needs to stay open: the link runs
 * in the notification listener's process.
 */
class MainActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var checks: LinearLayout
    private lateinit var pairInput: EditText
    private lateinit var pairHelp: TextView
    private lateinit var recent: TextView
    private val refresh: () -> Unit = { runOnUiThread { render() } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pad = dp(20)
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad * 2, pad, pad)
        }
        root.addView(TextView(this).apply {
            text = "phonectl"
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 28f)
        })
        status = TextView(this).apply {
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
            setPadding(0, dp(8), 0, dp(16))
        }
        root.addView(status)
        pairHelp = TextView(this).apply {
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
            setPadding(0, 0, 0, dp(16))
            text = "To pair with the laptop:\n" +
                "1. On the laptop, run:  phonectl setup --qr\n" +
                "2. Point the phone camera at the QR code and copy the text " +
                "(or tap Open if the camera offers it: that pairs at once).\n" +
                "3. Come back here: phonectl pairs by itself from the clipboard, " +
                "or tap \"Paste and pair\" below."
        }
        root.addView(pairHelp)
        root.addView(Button(this).apply {
            text = "Send clipboard to laptop"
            setOnClickListener { ClipSync.readNow("button"); toast("Clipboard sent") }
        })
        root.addView(Button(this).apply {
            text = "Reconnect"
            setOnClickListener { Link.kick("user") }
        })

        root.addView(heading("Setup"))
        checks = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(checks)

        root.addView(heading("Pairing"))
        pairInput = EditText(this).apply {
            hint = "phonectl:1;host=…  (from `phonectl setup`)"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
            isSingleLine = true
        }
        root.addView(pairInput)
        root.addView(Button(this).apply {
            text = "Pair"
            setOnClickListener { pair(pairInput.text.toString()) }
        })
        root.addView(Button(this).apply {
            text = "Paste and pair"
            setOnClickListener { if (!pairFromClipboard()) toast("The clipboard holds no phonectl pairing string") }
        })

        root.addView(heading("Updates"))
        root.addView(android.widget.Switch(this).apply {
            text = "Install updates of my own apps without asking (phone store, pinned signing keys, newer versions only)"
            isChecked = StoreUpdater.enabled
            setOnCheckedChangeListener { _, on -> StoreUpdater.enabled = on }
        })

        root.addView(heading("Recent laptop actions"))
        recent = TextView(this).apply {
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            setTextIsSelectable(true)
        }
        root.addView(recent)

        setContentView(ScrollView(this).apply { addView(root) })
        handlePairIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handlePairIntent(intent)
    }

    /** `--es pair <string>` from `phonectl setup` over ADB, or a scanned `phonectl:1;…` link. */
    private fun handlePairIntent(intent: Intent?) {
        intent ?: return
        // Any app or web page can send this intent, so it never pairs on its
        // own: the owner confirms the laptop's name and address first.
        intent.getStringExtra("pair")?.let { confirmPair(it); return }
        if (intent.action == Intent.ACTION_VIEW && intent.data?.scheme == "phonectl") {
            intent.dataString?.let { confirmPair(Uri.decode(it)) }
        }
    }

    private fun confirmPair(text: String) {
        val config = Config.parse(text) ?: run {
            toast("That is not a phonectl pairing string")
            return
        }
        val replacing = Link.config?.let { "\n\nThis replaces the pairing with ${it.name}." } ?: ""
        AlertDialog.Builder(this)
            .setTitle("Pair with this laptop?")
            .setMessage("${config.name} (${config.host})\n\nIt will receive your clipboard and notifications and can send commands to this phone.$replacing")
            .setPositiveButton("Pair") { _, _ -> pair(text) }
            .setNegativeButton("Cancel", null)
            .show()
    }

    /**
     * Pairs from a pairing string on the clipboard (copied from the laptop's
     * QR code), then clears it: it is the link key.
     */
    private fun pairFromClipboard(): Boolean {
        val clipboard = getSystemService(ClipboardManager::class.java)
        val text = try {
            clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()
        } catch (_: Exception) {
            null
        }?.trim() ?: return false
        if (Config.parse(text) == null) return false
        pair(text)
        try { clipboard.clearPrimaryClip() } catch (_: Exception) {}
        return true
    }

    private fun offerClipboardPairing(): Boolean {
        val clipboard = getSystemService(ClipboardManager::class.java)
        val text = try {
            clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()
        } catch (_: Exception) {
            null
        }?.trim() ?: return false
        if (Config.parse(text) == null) return false
        try { clipboard.clearPrimaryClip() } catch (_: Exception) {}
        confirmPair(text)
        return true
    }

    override fun onResume() {
        super.onResume()
        Link.addListener(refresh)
        Actions.onChange = refresh
        StatusMonitor.registerTelephony()
        ClipSync.startWatcher()
        render()
    }

    override fun onPause() {
        super.onPause()
        Link.removeListener(refresh)
        Actions.onChange = null
    }

    private fun pair(text: String) {
        val config = Config.pair(this, text)
        if (config == null) {
            toast("That is not a phonectl pairing string")
            return
        }
        pairInput.setText("")
        toast("Paired with ${config.name}")
        Link.start(this)
        Link.kick("paired")
    }

    private fun render() {
        val config = Link.config
        status.text = when (Link.state) {
            Link.State.UNPAIRED -> "Not paired with a laptop yet."
            Link.State.CONNECTING -> "Connecting to ${config?.name}…"
            Link.State.CONNECTED -> "Connected to ${Link.session?.laptopName} over ${Link.session?.transport?.wire}"
            Link.State.DISCONNECTED -> "Not connected" + (Link.lastError?.let { "\n$it" } ?: "")
        }

        val actions = Actions.recent().asReversed().take(30)
        recent.text = if (actions.isEmpty()) "None yet. Every command the laptop sends is listed here." else actions.joinToString("\n") { it.line() }
        pairHelp.visibility = if (Link.state == Link.State.UNPAIRED) android.view.View.VISIBLE else android.view.View.GONE
        checks.removeAllViews()
        val power = getSystemService(PowerManager::class.java)
        check("Notification access", "Mirrors notifications and keeps the link running", NotifListener.connected || listenerEnabled()) {
            startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
                .putExtra(Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME, ComponentName(this, NotifListener::class.java).flattenToString()))
        }
        check("Battery: unrestricted", "Keeps the connection alive in Doze", power.isIgnoringBatteryOptimizations(packageName)) {
            @Suppress("BatteryLife")
            startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
        }
        check("Phone state", "Pauses laptop media during calls; shows 5G/LTE", granted(Manifest.permission.READ_PHONE_STATE)) {
            requestPermissions(arrayOf(Manifest.permission.READ_PHONE_STATE), 1)
        }
        check("Nearby devices (Bluetooth)", "Fallback link when there is no network", granted(Manifest.permission.BLUETOOTH_CONNECT)) {
            requestPermissions(arrayOf(Manifest.permission.BLUETOOTH_CONNECT), 2)
        }
        when {
            ShellClip.active -> check("Automatic clipboard", "Copies reach the laptop by themselves (started by the laptop)", true) {}
            AdbWindow.canToggle() -> check("Automatic clipboard", "Starts by itself when the laptop connects and the phone is on trusted Wi-Fi", false) {
                toast("The laptop starts it over Wireless debugging; nothing to do here")
            }
            Keyboard.isDefault(this) -> check("phonectl Keyboard", "Copies on the phone reach the laptop by themselves", true) {}
            else -> {
                check("Automatic clipboard", "One-time: run phonectl setup-clipboard on the laptop", false) {
                    toast("Run on the laptop: phonectl setup-clipboard")
                }
                if (granted(Manifest.permission.READ_LOGS)) {
                    check("Display over other apps", "Lets the log-based fallback read a new clip", Settings.canDrawOverlays(this)) {
                        startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
                    }
                    check("Clipboard log fallback", "Android asks once after every restart of the app", ClipSync.logAccess) {
                        ClipSync.startWatcher()
                    }
                }
            }
        }
    }

    private fun check(title: String, detail: String, ok: Boolean, fix: () -> Unit) {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, dp(6), 0, dp(6))
        }
        row.addView(TextView(this).apply {
            text = if (ok) "✓" else "✗"
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
            alpha = if (ok) 0.6f else 1f
            setPadding(0, 0, dp(12), 0)
        })
        val texts = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        texts.addView(TextView(this).apply { text = title; setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f) })
        texts.addView(TextView(this).apply { text = detail; alpha = 0.6f })
        row.addView(texts, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
        if (!ok) row.setOnClickListener { fix() }
        checks.addView(row)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        StatusMonitor.registerTelephony()
        render()
    }

    private fun listenerEnabled(): Boolean =
        Settings.Secure.getString(contentResolver, "enabled_notification_listeners")
            ?.contains(ComponentName(this, NotifListener::class.java).flattenToString()) == true

    private fun granted(p: String) = checkSelfPermission(p) == PackageManager.PERMISSION_GRANTED

    private fun heading(text: String) = TextView(this).apply {
        this.text = text
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
        alpha = 0.6f
        isAllCaps = true
        setPadding(0, dp(24), 0, dp(4))
    }

    private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_SHORT).show()
    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus) return
        // Unpaired and a pairing string was just copied from the QR code:
        // offer it (another app may have put it there, so ask first).
        if (Link.config == null && offerClipboardPairing()) return
        ClipSync.readNow("app")
    }
}
