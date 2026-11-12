package com.daanh.phonectl

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class ActionsTest {
    private fun sources(): File {
        val candidates = listOf("src/main/java/com/daanh/phonectl", "app/src/main/java/com/daanh/phonectl")
        return candidates.map(::File).first { it.isDirectory }
    }

    @Test
    fun rateLimitIsPerMethodAndSlides() {
        val limiter = RateLimiter { if (it == "ring") 2 else 0 }
        assertTrue(limiter.allow("ring", 0))
        assertTrue(limiter.allow("ring", 1_000))
        assertFalse(limiter.allow("ring", 2_000))
        assertFalse(limiter.allow("other", 0))
        assertTrue(limiter.allow("ring", 60_000))
        assertFalse(limiter.allow("ring", 60_500))
        assertTrue(limiter.allow("ring", 61_000))
    }

    @Test
    fun unknownMethodsAreRefused() {
        assertEquals("unknown method", Actions.admit("pm_install_anything", 0))
        assertEquals("unknown method", Actions.admit("", 0))
    }

    @Test
    fun harmfulCommandsAreMarked() {
        val harmful = Actions.RULES.filterValues { it.risk == Actions.Risk.HARMFUL }.keys
        assertTrue("type_text" in harmful)
        for ((method, rule) in Actions.RULES) {
            assertTrue("$method needs a rate limit", rule.perMinute in 1..120)
        }
    }

    @Test
    fun recentLogFoldsRepeatsAndCaps() {
        val log = RecentLog(3)
        log.add(0, "ring", "ring", "done")
        log.add(10_000, "ring", "ring", "done")
        assertEquals(1, log.entries().size)
        assertEquals(2, log.entries()[0].count)
        log.add(20_000, "ring", "ring", "refused: rate limit")
        log.add(30_000, "diag", "log", "done")
        log.add(40_000, "icon", "icon", "done")
        assertEquals(listOf("ring", "diag", "icon"), log.entries().map { it.method })
        val copy = RecentLog(3).apply { load(log.toJson()) }
        assertEquals(log.entries().map { it.line() }, copy.entries().map { it.line() })
    }

    /** Link checks the gate before any feature sees a call. */
    @Test
    fun linkAdmitsBeforeDispatch() {
        val link = File(sources(), "Link.kt").readText()
        val admit = link.indexOf("when (val admission = Actions.admit(method, params, fromAgent = fromAgent))")
        val loop = link.indexOf("f.onCall(method, params)")
        assertTrue(admit in 0 until loop)
        // Features run only through invoke(): from the gate's Run branch, or after a tap on the phone.
        val callers = Regex("""\binvoke\(method, params\)""").findAll(link).count() +
            Regex("""Link\.invoke\(""").findAll(sources().listFiles()!!.joinToString("\n") { it.readText() }).count()
        assertEquals(2, callers)
        assertTrue(link.contains("is Actions.Admission.Run -> invoke(method, params)"))
        assertEquals(1, Regex("""f\.onCall\(""").findAll(link).count())
    }

    /** Every method a feature answers is listed in Actions.RULES (otherwise it could never run, or was added without a limit). */
    @Test
    fun everyHandledMethodHasARule() {
        val handled = mutableSetOf<String>()
        for (file in sources().listFiles()!!.filter { it.name.endsWith(".kt") }) {
            val lines = file.readLines()
            for ((i, line) in lines.withIndex()) {
                Regex("""method != "([a-z_]+)"""").findAll(line).forEach { handled += it.groupValues[1] }
                if (!line.contains("when (method)")) continue
                val indent = line.indexOfFirst { it != ' ' } + 4
                for (next in lines.drop(i + 1)) {
                    val lead = next.indexOfFirst { it != ' ' }
                    if (lead in 0 until indent && next.isNotBlank()) break
                    if (lead == indent && next.trimStart().startsWith("\"") && "->" in next) {
                        Regex(""""([a-z_]+)"""").findAll(next.substringBefore("->")).forEach { handled += it.groupValues[1] }
                    }
                }
            }
        }
        assertTrue(handled.size > 5)
        assertEquals(emptySet<String>(), handled - Actions.RULES.keys)
    }

    /** Locked phone (no keyguard in a unit test counts as locked): hotspot on asks, off runs, typing is refused. */
    @Test
    fun lockedPhoneAsksOrRefuses() {
        val on = org.json.JSONObject().put("on", true)
        val off = org.json.JSONObject().put("on", false)
        assertTrue(Actions.admit("hotspot", on, 1_000_000) is Actions.Admission.Ask)
        assertTrue(Actions.admit("hotspot", off, 1_000_001) is Actions.Admission.Run)
        assertTrue(Actions.admit("hotspot", null, 1_000_002) is Actions.Admission.Run)
        val typed = Actions.admit("type_text", org.json.JSONObject().put("text", "x"), 1_000_003)
        assertTrue(typed is Actions.Admission.Refuse && typed.reason == "phone is locked")
        assertTrue(Actions.admit("dnd", on, 1_000_004) is Actions.Admission.Run)
    }

    /** The laptop cannot press phonectl's own buttons (confirmations) or act on notifications it was not shown. */
    @Test
    fun laptopCannotTapOwnNotifications() {
        val listener = File(sources(), "NotifListener.kt").readText()
        assertTrue(listener.contains("if (!mirrored.containsKey(key)) throw"))
        assertTrue(listener.contains("if (sbn.packageName == context.packageName) throw"))
        assertTrue(listener.contains("if (sbn.packageName == context.packageName && n.channelId != SetupNotice.TEST_CHANNEL) return null"))
        val confirm = File(sources(), "Confirm.kt").readText()
        assertTrue(confirm.contains("MessageDigest.isEqual"))
        assertTrue(confirm.contains("pending = null"))
        assertTrue(confirm.contains("now - p.at > TTL_MS"))
    }

    @Test
    fun hotspotAndDndTakeOnlyABoolean() {
        for (name in listOf("Hotspot.kt", "Dnd.kt")) {
            val s = File(sources(), name).readText()
            assertTrue(name, s.contains("""params.get("on") as? Boolean ?: throw"""))
        }
        val hotspot = File(sources(), "Hotspot.kt").readText()
        assertTrue(hotspot.contains("ShellClip.borrow(Borrow.TETHER)"))
        assertTrue(hotspot.contains("ShellClip.giveBack(Borrow.TETHER"))
        assertTrue("hotspot config is never touched", !hotspot.contains("setSoftApConfiguration"))
    }

    /** AI agents on the laptop never drive confirm or harmful methods without a tap, even on an unlocked phone. */
    @Test
    fun agentCallsAlwaysAsk() {
        val on = org.json.JSONObject().put("on", true)
        val off = org.json.JSONObject().put("on", false)
        assertTrue(Actions.admit("hotspot", off, 2_000_000, fromAgent = true) is Actions.Admission.Ask)
        assertTrue(Actions.admit("hotspot", on, 2_000_001, fromAgent = true) is Actions.Admission.Ask)
        assertTrue(Actions.admit("ring", null, 2_000_002, fromAgent = true) is Actions.Admission.Run)
        assertTrue(Actions.admit("pm_install", null, 2_000_003, fromAgent = true) is Actions.Admission.Refuse)
        val link = File(sources(), "Link.kt").readText()
        assertTrue(link.contains("""message.optString("origin") == "agent""""))
    }

    /** Typing: plain text only, bounded, and only these keys. */
    @Test
    fun typingIsBounded() {
        val keyboard = File(sources(), "Keyboard.kt").readText()
        assertTrue(keyboard.contains("text.length > MAX_TYPED"))
        assertTrue(keyboard.contains("it.isISOControl() && it != '\\n' && it != '\\t'"))
        assertTrue(keyboard.contains("key !in ShellClip.KEYS"))
        val dir = sources().listFiles()!!.filter { it.readText().contains("borrow(Borrow.INJECT)") }.map { it.name }
        assertEquals(listOf("ShellWatcher.kt"), dir)
    }

    @Test
    fun onlyWebLinksAreAccepted() {
        assertEquals("https://example.org/a?b=c", Links.webLink(" https://example.org/a?b=c\n"))
        assertEquals("http://x.nl", Links.webLink("http://x.nl"))
        for (bad in listOf("intent://x#Intent;end", "javascript:alert(1)", "file:///sdcard/x", "content://x", "https://a b",
            "see https://x.nl", "https://", "HTTPS:/x", "https://x.nl\u0000", "https://" + "a".repeat(2050))) {
            assertEquals(bad, null, Links.webLink(bad))
        }
    }
}
