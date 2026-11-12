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
        val admit = link.indexOf("Actions.admit(method)")
        val loop = link.indexOf("f.onCall(method, params)")
        assertTrue(admit in 0 until loop)
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
}
