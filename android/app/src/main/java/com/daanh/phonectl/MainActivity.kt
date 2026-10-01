package com.daanh.phonectl

import android.Manifest
import android.app.Activity
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

        setContentView(ScrollView(this).apply { addView(root) })
        intent?.getStringExtra("pair")?.let { pair(it) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.getStringExtra("pair")?.let { pair(it) }
    }

    override fun onResume() {
        super.onResume()
        Link.addListener(refresh)
        StatusMonitor.registerTelephony()
        ClipSync.startWatcher()
        render()
    }

    override fun onPause() {
        super.onPause()
        Link.removeListener(refresh)
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
            Link.State.UNPAIRED -> "Not paired. Run `phonectl setup` on the laptop."
            Link.State.CONNECTING -> "Connecting to ${config?.name}…"
            Link.State.CONNECTED -> "Connected to ${Link.session?.laptopName} over ${Link.session?.transport?.wire}"
            Link.State.DISCONNECTED -> "Not connected" + (Link.lastError?.let { "\n$it" } ?: "")
        }

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
        check("Display over other apps", "Lets automatic clipboard sync read a new clip", Settings.canDrawOverlays(this)) {
            startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
        }
        if (!granted(Manifest.permission.READ_LOGS)) {
            check("Clipboard log access", "One-time over ADB: phonectl setup does it", false) {
                toast("Run on the laptop: phonectl setup")
            }
        } else {
            check("Automatic clipboard", "Android asks once after every restart of the app", ClipSync.logAccess) {
                ClipSync.startWatcher()
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
        if (hasFocus) ClipSync.readNow("app")
    }
}
