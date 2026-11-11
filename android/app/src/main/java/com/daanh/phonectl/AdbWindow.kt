package com.daanh.phonectl

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import org.json.JSONObject
import java.io.DataInputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

/**
 * Short wireless-debugging windows, so the laptop can (re)start [ShellWatcher]
 * without anyone touching the phone.
 *
 * With WRITE_SECURE_SETTINGS (granted once by `phonectl setup-clipboard`
 * with `pm grant`; it survives reboots and updates) the app may switch
 * Settings.Global "adb_wifi_enabled" itself. The laptop asks for a window
 * (`adb_open`), connects with its own paired key, runs `am instrument`, and
 * the restarted process switches debugging off again as soon as the watcher
 * runs. A window this app opened never stays open longer than [MAX_OPEN_MS],
 * even if the laptop goes away halfway; a window someone else opened is left
 * alone. Android only allows wireless debugging on Wi-Fi networks the owner
 * has trusted ("Always allow on this network"), so on other networks the
 * window simply does not open.
 */
object AdbWindow : Link.Feature {
    private const val PREFS = "adb_window"
    private const val KEY_OPENED_AT = "opened_at"
    private const val SETTING = "adb_wifi_enabled"
    private const val MAX_OPEN_MS = 2 * 60_000L
    private const val PORT_WAIT_MS = 20_000L

    private lateinit var context: Context
    private val main = Handler(Looper.getMainLooper())
    private val autoClose = Runnable { closeIfOurs("time limit") }
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "adb-window") }

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        // A window left open by a previous process (the laptop's `am
        // instrument` kills the old one): close it when it is overdue, or
        // keep the deadline running.
        val openedAt = openedAt()
        if (openedAt > 0) {
            val left = openedAt + MAX_OPEN_MS - System.currentTimeMillis()
            if (left <= 0 || left > MAX_OPEN_MS) closeIfOurs("left open") else main.postDelayed(autoClose, left)
        }
    }

    fun canToggle(): Boolean =
        ::context.isInitialized && context.checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) == PackageManager.PERMISSION_GRANTED

    private fun prefs() = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private fun openedAt() = prefs().getLong(KEY_OPENED_AT, 0L)
    fun isOn(): Boolean = Settings.Global.getInt(context.contentResolver, SETTING, 0) == 1

    private fun onWifi(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        @Suppress("DEPRECATION")
        return cm.allNetworks.any { cm.getNetworkCapabilities(it)?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true }
    }

    /**
     * Switches wireless debugging on (if needed) and finds adbd's port.
     * Result: {"port": n, "opened": true|false} or an error.
     */
    private fun open(): JSONObject {
        if (!canToggle()) throw IllegalStateException("phonectl may not switch wireless debugging (run phonectl setup-clipboard)")
        if (!onWifi()) throw IllegalStateException("phone is not on Wi-Fi")
        var opened = false
        if (!isOn()) {
            prefs().edit().putLong(KEY_OPENED_AT, System.currentTimeMillis()).commit()
            main.removeCallbacks(autoClose)
            main.postDelayed(autoClose, MAX_OPEN_MS)
            Settings.Global.putInt(context.contentResolver, SETTING, 1)
            opened = true
            Diag.log("wireless debugging on for the laptop")
        }
        val deadline = System.currentTimeMillis() + PORT_WAIT_MS
        while (true) {
            val port = findAdbdPort()
            if (port != null) return JSONObject().put("port", port).put("opened", opened)
            if (!isOn()) {
                // Android switched it back off: untrusted network or no Wi-Fi.
                prefs().edit().remove(KEY_OPENED_AT).commit()
                throw IllegalStateException("wireless debugging did not stay on (network not trusted for debugging?)")
            }
            if (System.currentTimeMillis() > deadline) {
                closeIfOurs("no port")
                throw IllegalStateException("adbd did not start listening")
            }
            Thread.sleep(500)
        }
    }

    /** Switches wireless debugging off, but only if this app switched it on. */
    fun closeIfOurs(reason: String) {
        if (!::context.isInitialized) return
        main.removeCallbacks(autoClose)
        if (openedAt() == 0L) return
        prefs().edit().remove(KEY_OPENED_AT).commit()
        try {
            Settings.Global.putInt(context.contentResolver, SETTING, 0)
            Diag.log("wireless debugging off ($reason)")
        } catch (e: SecurityException) {
            Diag.log("cannot switch wireless debugging off: ${e.javaClass.simpleName}")
        }
    }

    /**
     * adbd's wireless port is random and not readable by apps (no property,
     * /proc/net is closed to apps), so look for it: a local connect to a
     * closed port fails at once, and the one that answers an ADB CNXN with
     * STLS is adbd's TLS port. Takes well under a second.
     */
    private fun findAdbdPort(): Int? {
        val loopback = InetAddress.getLoopbackAddress()
        val ranges = listOf(32768..60999, 1024..32767, 61000..65535)
        for (range in ranges) {
            val next = AtomicInteger(range.first)
            val found = AtomicInteger(0)
            val threads = (1..8).map {
                Thread {
                    while (found.get() == 0) {
                        val port = next.getAndIncrement()
                        if (port > range.last) break
                        if (isAdbd(loopback, port)) found.compareAndSet(0, port)
                    }
                }.apply { start() }
            }
            threads.forEach { it.join() }
            if (found.get() != 0) return found.get()
        }
        return null
    }

    private fun isAdbd(host: InetAddress, port: Int): Boolean = try {
        Socket().use { socket ->
            socket.connect(InetSocketAddress(host, port), 100)
            socket.soTimeout = 400
            socket.getOutputStream().write(cnxn())
            val header = ByteArray(24)
            DataInputStream(socket.getInputStream()).readFully(header)
            ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN).int == A_STLS
        }
    } catch (_: Exception) {
        false
    }

    private const val A_CNXN = 0x4e584e43
    private const val A_STLS = 0x534c5453

    private fun cnxn(): ByteArray {
        val payload = "host::\u0000".toByteArray()
        val buffer = ByteBuffer.allocate(24 + payload.size).order(ByteOrder.LITTLE_ENDIAN)
        buffer.putInt(A_CNXN).putInt(0x01000001).putInt(256 * 1024).putInt(payload.size)
            .putInt(payload.sumOf { it.toInt() and 0xff }).putInt(A_CNXN.inv())
        buffer.put(payload)
        return buffer.array()
    }

    override fun onCall(method: String, params: JSONObject?): Any? = when (method) {
        "adb_open" -> CompletableFuture.supplyAsync({ open() }, worker)
        "adb_close" -> {
            closeIfOurs("laptop done")
            Unit
        }
        else -> null
    }
}
