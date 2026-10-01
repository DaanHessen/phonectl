package com.daanh.phonectl

import android.os.SystemClock
import android.util.Log
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

enum class Transport(val wire: String) { TAILSCALE("tailscale"), BLUETOOTH("bluetooth") }

/**
 * One authenticated connection to the laptop over some transport.
 *
 * Reading happens on a dedicated thread that blocks in read() (no CPU while
 * idle); writing goes through a single-thread executor so callers never block
 * on the network.
 */
class Session(
    val transport: Transport,
    private val input: InputStream,
    private val output: OutputStream,
    private val closer: Closeable,
) {
    private val reader = LineReader(BufferedInputStream(input, 16 * 1024))
    private val writer: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "link-write") }
    private val closed = AtomicBoolean(false)
    @Volatile var lastReceived = SystemClock.elapsedRealtime()
        private set
    var laptopName: String = "laptop"
        private set

    /**
     * Runs the mutual HMAC handshake. Called on the connecting thread with a
     * read timeout already set on the socket where the transport supports it.
     */
    fun handshake(config: Config, hello: JSONObject) {
        val phoneNonce = Protocol.nonce()
        writeNow(hello.put("type", "hello").put("protocol", Protocol.VERSION).put("nonce", phoneNonce))
        val challenge = readJson() ?: throw IOException("closed during handshake")
        if (challenge.optString("type") == "error") throw IOException("laptop refused: ${challenge.optString("message")}")
        if (challenge.optString("type") != "challenge") throw IOException("unexpected ${challenge.optString("type")}")
        val laptopNonce = challenge.getString("nonce")
        val expected = Protocol.proof(config.key, "laptop", phoneNonce, laptopNonce)
        if (!Protocol.sameProof(expected, challenge.optString("proof"))) throw IOException("laptop failed authentication")
        writeNow(JSONObject().put("type", "auth").put("proof", Protocol.proof(config.key, "phone", phoneNonce, laptopNonce)))
        val ready = readJson() ?: throw IOException("closed during handshake")
        if (ready.optString("type") != "ready") throw IOException("laptop rejected authentication")
        laptopName = ready.optString("name", config.name)
    }

    /** Blocks until the connection ends; hands each message to [onMessage]. */
    fun readLoop(onMessage: (JSONObject) -> Unit) {
        try {
            while (!closed.get()) {
                val message = readJson() ?: break
                onMessage(message)
            }
        } catch (e: IOException) {
            if (!closed.get()) Log.i(TAG, "${transport.wire} read ended: ${e.javaClass.simpleName}")
        } finally {
            close()
        }
    }

    fun send(message: JSONObject) {
        if (closed.get()) return
        try {
            writer.execute {
                try {
                    writeNow(message)
                } catch (e: IOException) {
                    Log.i(TAG, "${transport.wire} write failed: ${e.javaClass.simpleName}")
                    close()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {
        }
    }

    val isOpen: Boolean get() = !closed.get()

    fun close() {
        if (!closed.compareAndSet(false, true)) return
        writer.shutdownNow()
        try { closer.close() } catch (_: IOException) {}
    }

    private fun writeNow(message: JSONObject) {
        val bytes = (message.toString() + "\n").toByteArray()
        synchronized(output) {
            output.write(bytes)
            output.flush()
        }
    }

    private fun readJson(): JSONObject? {
        val line = reader.readLine() ?: return null
        lastReceived = SystemClock.elapsedRealtime()
        return try {
            JSONObject(line)
        } catch (e: org.json.JSONException) {
            throw IOException("malformed message")
        }
    }

    /** Minimal UTF-8 line reader with a hard size cap. */
    private class LineReader(private val input: InputStream) {
        private val buffer = ByteArrayOutputStream(1024)
        fun readLine(): String? {
            buffer.reset()
            while (true) {
                val b = input.read()
                if (b < 0) return if (buffer.size() == 0) null else throw IOException("truncated line")
                if (b == '\n'.code) return buffer.toString(Charsets.UTF_8.name())
                buffer.write(b)
                if (buffer.size() > Protocol.MAX_LINE) throw IOException("line too long")
            }
        }
    }

    companion object { private const val TAG = "phonectl" }
}
