package com.daanh.phonectl

import android.content.Context
import android.net.TetheringInterface
import android.net.TetheringManager
import org.json.JSONObject
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The phone's Wi-Fi hotspot: its state for the laptop's status, and
 * `phonectl hotspot on|off`. Switching needs TETHER_PRIVILEGED, which the
 * shell holds; it is borrowed only for the start or stop call and its answer
 * ([Borrow.TETHER], 30 seconds at most). The saved hotspot name and password
 * are used as they are; nothing here can read or change them.
 */
object Hotspot : Link.Feature {
    private lateinit var context: Context
    private val executor = Executor { Link.handler.post(it) }

    @Volatile var on = false
        private set
    @Volatile var lastError: String? = null
        private set

    private val events = object : TetheringManager.TetheringEventCallback {
        override fun onTetheredInterfacesChanged(interfaces: Set<TetheringInterface>) {
            val now = interfaces.any { it.type == TetheringManager.TETHERING_WIFI }
            if (now != on) {
                on = now
                Diag.log("hotspot ${if (now) "on" else "off"}")
                StatusMonitor.refresh()
            }
        }
    }

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
        Link.handler.post {
            try {
                context.getSystemService(TetheringManager::class.java)?.registerTetheringEventCallback(executor, events)
            } catch (e: Exception) {
                Diag.log("hotspot state unavailable: ${e.javaClass.simpleName}")
            }
        }
    }

    fun status(): JSONObject = JSONObject()
        .put("on", on)
        .put("can_toggle", ShellClip.active)
        .put("error", lastError ?: JSONObject.NULL)

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "hotspot") return null
        if (params == null || !params.has("on")) throw IllegalArgumentException("on")
        val wanted = params.get("on") as? Boolean ?: throw IllegalArgumentException("on must be true or false")
        return set(wanted)
    }

    private fun set(wanted: Boolean): CompletableFuture<Any> {
        val result = CompletableFuture<Any>()
        if (wanted == on) return result.apply { complete(Unit) }
        if (!ShellClip.borrow(Borrow.TETHER)) {
            return result.apply { completeExceptionally(IllegalStateException("switch it on the phone: the clipboard watcher (shell identity) is not running")) }
        }
        val finished = AtomicBoolean(false)
        fun finish(error: String?) {
            if (!finished.compareAndSet(false, true)) return
            ShellClip.giveBack(Borrow.TETHER, if (error == null) "done" else "failed")
            lastError = error
            StatusMonitor.refresh()
            if (error == null) result.complete(Unit) else result.completeExceptionally(IllegalStateException(error))
        }
        Link.handler.postDelayed({ finish("no answer from Android within 20 s") }, 20_000)
        try {
            val tm = context.getSystemService(TetheringManager::class.java) ?: throw IllegalStateException("no tethering service")
            val request = TetheringManager.TetheringRequest.Builder(TetheringManager.TETHERING_WIFI).build()
            if (wanted) {
                tm.startTethering(request, executor, object : TetheringManager.StartTetheringCallback {
                    override fun onTetheringStarted() = finish(null)
                    override fun onTetheringFailed(error: Int) = finish("hotspot did not start (error $error)")
                })
            } else {
                tm.stopTethering(request, executor, object : TetheringManager.StopTetheringCallback {
                    override fun onStopTetheringSucceeded() = finish(null)
                    override fun onStopTetheringFailed(error: Int) = finish("hotspot did not stop (error $error)")
                })
            }
        } catch (e: Exception) {
            finish(e.message ?: e.javaClass.simpleName)
        }
        return result
    }
}
