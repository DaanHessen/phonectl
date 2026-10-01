package com.daanh.phonectl

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Vectors shared with `crates/phone/src/link.rs`, so both sides agree byte for byte. */
class ProtocolTest {
    @Test fun proofMatchesRust() {
        val key = ByteArray(32) { 1 }
        assertEquals(
            "1a32a59da8240ccd7046452fe7432aa0181f562777dc027e98686f2269a94d55",
            Protocol.proof(key, "phone", "00", "11"),
        )
    }

    @Test fun clipHashMatchesRust() {
        assertEquals("2cf24dba5fb0a30e26e83b2ac5b9e29e", Protocol.clipHash("hello"))
    }

    @Test fun pokeHmacMatchesRust() {
        val packet = "70686f6e6563746c2d706f6b65000001a0c4506c0080866bc5c74d4432c01ab8f26a0a3873cb8d640e6f6a6d6411ee235fd9ec2d64"
            .chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val body = packet.copyOfRange(0, 21)
        assertEquals(Protocol.hex(packet.copyOfRange(21, 53)), Protocol.hex(Protocol.hmac(ByteArray(32) { 3 }, body)))
        // Valid MAC but far outside the time window: rejected.
        Poke.setKey(ByteArray(32) { 3 })
        assertFalse(Poke.valid(packet))
    }

    @Test fun pairingStringParses() {
        val key = java.util.Base64.getEncoder().encodeToString(ByteArray(32) { 9 })
        val config = Config.parse("phonectl:1;host=100.101.102.103;port=47201;name=omarchy;bt=00:11:22:33:44:55;key=$key")
        assertNotNull(config)
        assertEquals("100.101.102.103", config!!.host)
        assertEquals("00:11:22:33:44:55", config.bt)
        assertEquals("omarchy", config.name)
        assertTrue(config.key.all { it == 9.toByte() })
        assertNull(Config.parse("phonectl:1;host=x;key=c2hvcnQ="))  // key too short
        assertNull(Config.parse("something else"))
    }
}
