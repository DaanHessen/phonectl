package com.daanh.phonectl

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class StoreUpdaterTest {
    private fun source(name: String): String {
        val candidates = listOf("src/main/java/com/daanh/phonectl", "app/src/main/java/com/daanh/phonectl")
        return File(candidates.map(::File).first { it.isDirectory }, name).readText()
    }

    @Test
    fun everyPackageHasOneFullPinAndPhonectlIsLast() {
        for ((pkg, pin) in OwnApps.PINS) {
            assertTrue("$pkg pin is not a SHA-256", Regex("[0-9a-f]{64}").matches(pin))
        }
        assertEquals(OwnApps.PHONECTL, OwnApps.PACKAGES.last())
    }

    /** The APK is checked (package, pinned key, newer version) before the install permission is borrowed. */
    @Test
    fun storeInstallVerifiesFirst() {
        val s = source("StoreUpdater.kt")
        val verify = s.indexOf("OwnApps.verify(context, file, pkg)")
        val install = s.indexOf("install(pkg, file)")
        val borrow = s.indexOf("ShellClip.borrow(Borrow.INSTALL)")
        assertTrue(verify in 0 until install)
        assertTrue(borrow > 0)
        val dir = File(listOf("src/main/java/com/daanh/phonectl", "app/src/main/java/com/daanh/phonectl").map(::File).first { it.isDirectory }.path)
        val borrowers = dir.listFiles()!!.filter { it.readText().contains("borrow(Borrow.INSTALL)") }.map { it.name }
        assertEquals("only StoreUpdater borrows the install permission", listOf("StoreUpdater.kt"), borrowers)
    }

    /** Every way an install ends gives the permission back. */
    @Test
    fun installPermissionIsReturned() {
        assertTrue(source("Updater.kt").contains("ShellClip.giveBack(Borrow.INSTALL, \"install done\")"))
        assertTrue(source("StoreUpdater.kt").contains("ShellClip.giveBack(Borrow.INSTALL, \"install failed\")"))
        assertTrue(source("ShellWatcher.kt").contains("INSTALL(INSTALL_PACKAGES, 2 * 60_000L)"))
        assertTrue(source("ShellWatcher.kt").contains("main.postDelayed(limit, what.maxMs)"))
    }

    /** The laptop can ask for a check, never switch silent updates on or off. */
    @Test
    fun onlyThePhoneSwitchesSilentUpdates() {
        val dir = File(listOf("src/main/java/com/daanh/phonectl", "app/src/main/java/com/daanh/phonectl").map(::File).first { it.isDirectory }.path)
        val writers = dir.listFiles()!!.filter { it.readText().contains(Regex("""StoreUpdater\.enabled\s*=""")) }.map { it.name }
        assertEquals(listOf("MainActivity.kt"), writers)
    }

    @Test
    fun updatesUsePlainHttpOnlyToTheStore() {
        val candidates = listOf("src/main/res/xml/network_security_config.xml", "app/src/main/res/xml/network_security_config.xml")
        val xml = candidates.map(::File).first { it.exists() }.readText()
        assertTrue(xml.contains("<base-config cleartextTrafficPermitted=\"false\""))
        assertEquals(1, Regex("<domain ").findAll(xml).count())
        assertTrue(source("StoreUpdater.kt").contains("instanceFollowRedirects = false"))
    }
}
