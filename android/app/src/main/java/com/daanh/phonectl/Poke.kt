package com.daanh.phonectl

import android.content.Context
import android.util.Log
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.nio.ByteBuffer

/**
 * Listens for the laptop's "connect now" datagram on UDP 47202.
 *
 * The phone is the only side that dials, so after the laptop resumes from
 * suspend or restarts its daemon it sends one of these over Tailscale instead
 * of waiting for the phone's backoff timer. The thread sits in receive() and
 * costs nothing while idle.
 *
 * Datagram: "phonectl-poke" (13 bytes) + u64 big-endian unix millis + 32-byte
 * HMAC-SHA256 over the first 21 bytes. Stale or unauthenticated pokes are
 * ignored.
 */
object Poke {
    private const val MAGIC = "phonectl-poke"
    @Volatile private var key: ByteArray? = null
    private var started = false

    fun setKey(k: ByteArray) { key = k }

    fun start(context: Context) {
        if (started) return
        started = true
        Thread({ loop() }, "link-poke").apply { isDaemon = true }.start()
    }

    private fun loop() {
        while (true) {
            try {
                DatagramSocket(Protocol.POKE_PORT).use { socket ->
                    val buffer = ByteArray(128)
                    while (true) {
                        val packet = DatagramPacket(buffer, buffer.size)
                        socket.receive(packet)
                        if (valid(packet.data.copyOf(packet.length))) Link.kick("poke")
                    }
                }
            } catch (e: Exception) {
                Log.w("phonectl", "poke listener: ${e.javaClass.simpleName}")
                Thread.sleep(60_000)
            }
        }
    }

    fun valid(data: ByteArray): Boolean {
        val k = key ?: return false
        if (data.size != MAGIC.length + 8 + 32) return false
        if (String(data, 0, MAGIC.length) != MAGIC) return false
        val sent = ByteBuffer.wrap(data, MAGIC.length, 8).long
        if (Math.abs(System.currentTimeMillis() - sent) > 5 * 60_000) return false
        val body = data.copyOfRange(0, MAGIC.length + 8)
        val mac = Protocol.hmac(k, body)
        return java.security.MessageDigest.isEqual(mac, data.copyOfRange(MAGIC.length + 8, data.size))
    }
}
