package com.daanh.phonectl

import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * The link protocol, phone side. Mirrors `crates/phone/src/link/protocol.rs`.
 *
 * Newline-delimited JSON. The phone always dials; the laptop answers.
 *
 *   phone  → {"type":"hello","protocol":1,"nonce":n1,"device":{…}}
 *   laptop → {"type":"challenge","nonce":n2,"proof":hmac("laptop",n1,n2)}
 *   phone  → {"type":"auth","proof":hmac("phone",n1,n2)}
 *   laptop → {"type":"ready","name":"omarchy"}
 *
 * After that both sides send `event`, `call`/`result` and `ping`/`pong`.
 * The transport (Tailscale TCP or Bluetooth RFCOMM) already encrypts; the
 * HMAC exchange proves both ends hold the key from pairing.
 */
object Protocol {
    const val VERSION = 1
    const val TCP_PORT = 47201
    const val POKE_PORT = 47202
    /** RFCOMM service the laptop registers with BlueZ. */
    const val BT_UUID = "8f0c6a1e-3b5d-4c8e-9a71-5d2c4e6b7a10"
    /** Longest line we accept (clipboard text is capped below this). */
    const val MAX_LINE = 2 * 1024 * 1024
    const val MAX_CLIP = 1024 * 1024

    private val random = SecureRandom()

    fun nonce(): String {
        val bytes = ByteArray(16)
        random.nextBytes(bytes)
        return hex(bytes)
    }

    fun proof(key: ByteArray, role: String, phoneNonce: String, laptopNonce: String): String =
        hex(hmac(key, "phonectl-auth|$role|$phoneNonce|$laptopNonce".toByteArray()))

    fun hmac(key: ByteArray, data: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(data)
    }

    /** Constant-time comparison of two hex proofs. */
    fun sameProof(a: String, b: String): Boolean =
        MessageDigest.isEqual(a.toByteArray(), b.toByteArray())

    /** Content hash used for clipboard loop prevention; identical on both sides. */
    fun clipHash(text: String): String =
        hex(MessageDigest.getInstance("SHA-256").digest(text.toByteArray())).substring(0, 32)

    fun hex(bytes: ByteArray): String {
        val out = StringBuilder(bytes.size * 2)
        for (b in bytes) {
            val v = b.toInt() and 0xff
            out.append("0123456789abcdef"[v shr 4]).append("0123456789abcdef"[v and 15])
        }
        return out.toString()
    }

    fun event(topic: String, data: Any?): JSONObject =
        JSONObject().put("type", "event").put("topic", topic).put("data", data ?: JSONObject.NULL)

    fun result(id: Long, data: Any?, error: String? = null): JSONObject {
        val o = JSONObject().put("type", "result").put("id", id)
        if (error != null) o.put("error", error) else o.put("data", data ?: JSONObject.NULL)
        return o
    }
}
