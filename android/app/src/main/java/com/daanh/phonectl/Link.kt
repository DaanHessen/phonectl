package com.daanh.phonectl

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.HandlerThread
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import org.json.JSONObject
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.math.min

/**
 * Owns the single connection to the laptop: picks the transport, reconnects
 * with backoff, and routes messages to the features.
 *
 * Transport policy:
 *  - Tailscale TCP whenever the phone has any network. It is tried first on
 *    every attempt.
 *  - Bluetooth RFCOMM only when there is no network, or Tailscale has failed
 *    twice in a row (laptop offline from the tailnet but in radio range).
 *  - While on Bluetooth, a network change (or a 10 minute timer) retries
 *    Tailscale; on success the Bluetooth session is closed.
 *
 * Nothing here polls. Attempts are driven by: process start, network
 * callbacks, a UDP poke from the laptop, and backoff timers on the uptime
 * clock (which stops in deep sleep, so a sleeping phone is never woken just to
 * retry).
 */
@SuppressLint("StaticFieldLeak")
object Link {
    private const val TAG = "phonectl"

    enum class State { UNPAIRED, DISCONNECTED, CONNECTING, CONNECTED }

    interface Feature {
        /** A session became ready (also after every reconnect). */
        fun onConnected(session: Session) {}
        fun onDisconnected() {}
        /** Laptop → phone event. Return true when handled. */
        fun onEvent(topic: String, data: JSONObject?): Boolean = false
        /** Laptop → phone call. Return the result data, or throw. Null = not mine. */
        fun onCall(method: String, params: JSONObject?): Any? = null
    }

    private lateinit var context: Context
    private val thread = HandlerThread("link").apply { start() }
    val handler = Handler(thread.looper)
    private val features = CopyOnWriteArrayList<Feature>()
    private val listeners = CopyOnWriteArrayList<() -> Unit>()

    @Volatile var state = State.DISCONNECTED
        private set
    @Volatile var session: Session? = null
        private set
    @Volatile var lastError: String? = null
        private set
    @Volatile var config: Config? = null
        private set

    private var started = false
    private var attempting = false
    private var kickedDuringAttempt = false
    /**
     * The laptop said it is suspending. Until it pokes us (or the user asks),
     * only a slow timer retries: no point burning radio time on a closed lid.
     */
    @Volatile var laptopAsleep = false
        private set
    private var failures = 0
    private var ipFailures = 0
    private val retry = Runnable { attempt("backoff") }
    private val ipUpgrade = Runnable { if (session?.transport == Transport.BLUETOOTH) attempt("upgrade timer") }
    private val watchdog = object : Runnable {
        override fun run() {
            val s = session
            // The laptop pings every 4 minutes. Three missed pings (or a dead
            // socket the kernel never reported) means the link is gone.
            if (s != null && SystemClock.elapsedRealtime() - s.lastReceived > WATCHDOG_MS) {
                Diag.log("watchdog: no traffic, dropping ${s.transport.wire}")
                s.close()
            }
            handler.postDelayed(this, WATCHDOG_CHECK_MS)
        }
    }
    private var wakeLock: PowerManager.WakeLock? = null

    private const val BASE_BACKOFF_MS = 2_000L
    private const val MAX_BACKOFF_MS = 10 * 60_000L
    private const val BT_UPGRADE_MS = 10 * 60_000L
    private const val WATCHDOG_MS = 13 * 60_000L
    private const val WATCHDOG_CHECK_MS = 5 * 60_000L
    private const val ASLEEP_RETRY_MS = 30 * 60_000L
    private const val CONNECT_TIMEOUT_MS = 8_000

    fun register(feature: Feature) { features.addIfAbsent(feature) }
    fun addListener(listener: () -> Unit) { listeners.addIfAbsent(listener) }
    fun removeListener(listener: () -> Unit) { listeners.remove(listener) }
    private fun changed() { listeners.forEach { it() } }
    fun notifyListeners() = changed()

    /** Idempotent. Called from Application.onCreate and after pairing. */
    fun start(ctx: Context) {
        handler.post {
            if (!started) {
                context = ctx.applicationContext
                started = true
                wakeLock = (context.getSystemService(PowerManager::class.java))
                    .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "phonectl:connect")
                    .apply { setReferenceCounted(false) }
                val cm = context.getSystemService(ConnectivityManager::class.java)
                cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) = kick("network available")
                    override fun onLost(network: Network) = kick("network lost")
                })
                Poke.start(context)
                handler.postDelayed(watchdog, WATCHDOG_CHECK_MS)
            }
            config = Config.load(context)
            if (config == null) {
                state = State.UNPAIRED
                changed()
                return@post
            }
            Poke.setKey(config!!.key)
            attempt("start")
        }
    }

    /**
     * Something suggests the laptop may be reachable now (network change, poke,
     * user action). Resets the backoff and tries at once unless already on
     * Tailscale.
     */
    fun kick(reason: String) {
        handler.post {
            if (session?.transport == Transport.TAILSCALE) return@post
            if (laptopAsleep) {
                // Our own network changes say nothing about a sleeping laptop.
                if (reason.startsWith("network")) return@post
                laptopAsleep = false
            }
            failures = 0
            ipFailures = 0
            if (attempting) kickedDuringAttempt = true else attempt(reason)
        }
    }

    fun send(message: JSONObject): Boolean {
        val s = session ?: return false
        s.send(message)
        return true
    }

    fun event(topic: String, data: Any?): Boolean = send(Protocol.event(topic, data))

    private fun attempt(reason: String) {
        if (!started || attempting || config == null) return
        if (session?.transport == Transport.TAILSCALE) return
        handler.removeCallbacks(retry)
        attempting = true
        if (session == null) {
            state = State.CONNECTING
            changed()
        }
        val cfg = config!!
        // Keep the CPU up for the length of one attempt only: a poke or a
        // network change may arrive just as the phone is about to sleep.
        wakeLock?.acquire(30_000)
        Thread({
            val result = try {
                connect(cfg, reason)
            } finally {
                wakeLock?.let { if (it.isHeld) it.release() }
            }
            handler.post {
                attempting = false
                finished(result)
                if (kickedDuringAttempt) {
                    kickedDuringAttempt = false
                    if (session?.transport != Transport.TAILSCALE) attempt("kicked during attempt")
                }
            }
        }, "link-connect").start()
    }

    private sealed interface Outcome {
        data class Ok(val session: Session) : Outcome
        data class Failed(val error: String) : Outcome
    }

    /** Runs on the connect thread. Tries the transports in policy order. */
    private fun connect(cfg: Config, reason: String): Outcome {
        val onBluetooth = session?.transport == Transport.BLUETOOTH
        val network = hasNetwork()
        var error = "no network"
        if (network) {
            try {
                return Outcome.Ok(open(cfg, Transport.TAILSCALE))
            } catch (e: Exception) {
                error = "tailscale: ${describe(e)}"
                ipFailures++
            }
        }
        if (!onBluetooth && (!network || ipFailures >= 2) && cfg.bt != null && bluetoothUsable()) {
            try {
                return Outcome.Ok(open(cfg, Transport.BLUETOOTH))
            } catch (e: Exception) {
                error += "; bluetooth: ${describe(e)}"
            }
        }
        Diag.log("connect ($reason) failed: $error")
        return Outcome.Failed(error)
    }

    private fun open(cfg: Config, transport: Transport): Session {
        val session = when (transport) {
            Transport.TAILSCALE -> {
                val socket = Socket()
                try {
                    socket.connect(InetSocketAddress(cfg.host, cfg.port), CONNECT_TIMEOUT_MS)
                    socket.soTimeout = 10_000
                    socket.tcpNoDelay = true
                    Session(transport, socket.getInputStream(), socket.getOutputStream(), socket).also {
                        it.handshake(cfg, Hello.build(context))
                        socket.soTimeout = 0
                    }
                } catch (e: Exception) {
                    socket.close()
                    throw e
                }
            }
            Transport.BLUETOOTH -> {
                val adapter = context.getSystemService(BluetoothManager::class.java).adapter
                @SuppressLint("MissingPermission")
                val socket = adapter.getRemoteDevice(cfg.bt).createRfcommSocketToServiceRecord(UUID.fromString(Protocol.BT_UUID))
                try {
                    @SuppressLint("MissingPermission")
                    socket.connect()
                    // RFCOMM has no read timeout; a stalled handshake is cut by
                    // closing the socket from a timer.
                    val guard = Runnable { try { socket.close() } catch (_: IOException) {} }
                    handler.postDelayed(guard, 15_000)
                    try {
                        Session(transport, socket.inputStream, socket.outputStream, socket).also {
                            it.handshake(cfg, Hello.build(context))
                        }
                    } finally {
                        handler.removeCallbacks(guard)
                    }
                } catch (e: Exception) {
                    try { socket.close() } catch (_: IOException) {}
                    throw e
                }
            }
        }
        Diag.log("connected over ${transport.wire}")
        return session
    }

    private fun finished(outcome: Outcome) {
        when (outcome) {
            is Outcome.Ok -> adopt(outcome.session)
            is Outcome.Failed -> {
                lastError = outcome.error
                if (session == null) {
                    failures++
                    state = State.DISCONNECTED
                    val delay = if (laptopAsleep) ASLEEP_RETRY_MS
                        else min(MAX_BACKOFF_MS, BASE_BACKOFF_MS shl min(failures - 1, 12))
                    handler.postDelayed(retry, delay)
                } else {
                    // Still on Bluetooth; try Tailscale again later.
                    handler.removeCallbacks(ipUpgrade)
                    handler.postDelayed(ipUpgrade, BT_UPGRADE_MS)
                }
                changed()
            }
        }
    }

    private fun adopt(new: Session) {
        val old = session
        session = new
        failures = 0
        ipFailures = 0
        lastError = null
        laptopAsleep = false
        state = State.CONNECTED
        handler.removeCallbacks(retry)
        handler.removeCallbacks(ipUpgrade)
        if (new.transport == Transport.BLUETOOTH) handler.postDelayed(ipUpgrade, BT_UPGRADE_MS)
        old?.close()
        Thread({
            new.readLoop { message -> handler.post { dispatch(new, message) } }
            handler.post { lost(new) }
        }, "link-read-${new.transport.wire}").start()
        features.forEach { safely { it.onConnected(new) } }
        changed()
    }

    private fun lost(old: Session) {
        if (session !== old) return
        session = null
        state = State.DISCONNECTED
        features.forEach { safely { it.onDisconnected() } }
        changed()
        // A drop is usually a network change; try soon, then back off.
        failures = 0
        handler.postDelayed(retry, if (laptopAsleep) ASLEEP_RETRY_MS else BASE_BACKOFF_MS)
    }

    private fun dispatch(from: Session, message: JSONObject) {
        if (session !== from) return
        when (message.optString("type")) {
            "ping" -> from.send(JSONObject().put("type", "pong").put("id", message.opt("id")))
            "pong" -> {}
            "sleeping" -> {
                Diag.log("laptop is suspending")
                laptopAsleep = true
                from.close()
            }
            "event" -> {
                val topic = message.optString("topic")
                val data = message.optJSONObject("data")
                if (features.none { f -> safely(false) { f.onEvent(topic, data) } }) Log.d(TAG, "unhandled event $topic")
            }
            "call" -> {
                val id = message.optLong("id")
                val method = message.optString("method")
                val params = message.optJSONObject("params")
                var reply: JSONObject = Protocol.result(id, null, "unknown method $method")
                for (f in features) {
                    try {
                        val data = f.onCall(method, params) ?: continue
                        reply = Protocol.result(id, if (data == Unit) null else data)
                        break
                    } catch (e: Exception) {
                        reply = Protocol.result(id, null, e.message ?: e.javaClass.simpleName)
                        break
                    }
                }
                from.send(reply)
            }
        }
    }

    private fun hasNetwork(): Boolean {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork ?: return false) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private fun bluetoothUsable(): Boolean {
        if (context.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED) return false
        val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter ?: return false
        return adapter.isEnabled
    }

    private fun describe(e: Exception): String = when (e) {
        is java.net.SocketTimeoutException -> "timed out"
        is java.net.ConnectException -> "refused or unreachable"
        else -> e.message?.take(80) ?: e.javaClass.simpleName
    }

    private inline fun safely(block: () -> Unit) {
        try { block() } catch (e: Exception) { Log.w(TAG, "feature failed", e) }
    }

    private inline fun <T> safely(fallback: T, block: () -> T): T =
        try { block() } catch (e: Exception) { Log.w(TAG, "feature failed", e); fallback }
}
