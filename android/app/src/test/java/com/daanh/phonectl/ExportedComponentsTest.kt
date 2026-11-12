package com.daanh.phonectl

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element
import java.io.File
import javax.xml.parsers.DocumentBuilderFactory

/**
 * Fails when a component other apps can reach is added or loses its guard.
 * Every exported entry point must be listed here with the reason it is safe;
 * see docs/security.md.
 */
class ExportedComponentsTest {
    private val android = "http://schemas.android.com/apk/res/android"

    /** Exported on purpose, each guarded as described. */
    private val allowed = mapOf(
        // Launcher icon; pairing links it receives only open a confirm dialog.
        ".MainActivity" to null,
        // Share sheet target: sends shared text to the paired laptop only.
        ".ShareActivity" to null,
        // Only the system can bind these.
        ".NotifListener" to "android.permission.BIND_NOTIFICATION_LISTENER_SERVICE",
        ".ClipTile" to "android.permission.BIND_QUICK_SETTINGS_TILE",
        // Signature permission: only apps signed with our key.
        ".KeyboardBridge" to "com.daanh.phonectl.permission.BRIDGE",
    )

    private fun manifest(): Element {
        val candidates = listOf("src/main/AndroidManifest.xml", "app/src/main/AndroidManifest.xml")
        val file = candidates.map(::File).first { it.exists() }
        val factory = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
        return factory.newDocumentBuilder().parse(file).documentElement
    }

    private fun Element.children(tag: String): List<Element> {
        val nodes = getElementsByTagName(tag)
        return (0 until nodes.length).map { nodes.item(it) as Element }
    }

    @Test
    fun onlyKnownComponentsAreExported() {
        val root = manifest()
        val exported = listOf("activity", "activity-alias", "service", "receiver", "provider")
            .flatMap { root.children(it) }
            .filter { c ->
                val flag = c.getAttributeNS(android, "exported")
                flag == "true" || (flag.isEmpty() && c.getElementsByTagName("intent-filter").length > 0)
            }
        val names = exported.map { it.getAttributeNS(android, "name") }.toSet()
        assertEquals("exported components changed; update docs/security.md and this list", allowed.keys, names)
        for (component in exported) {
            val name = component.getAttributeNS(android, "name")
            val expected = allowed[name] ?: continue
            assertEquals("$name lost its permission guard", expected, component.getAttributeNS(android, "permission"))
        }
    }

    @Test
    fun noContentProviders() {
        assertTrue("content providers are not allowed", manifest().children("provider").isEmpty())
    }

    @Test
    fun bridgePermissionIsSignatureOnly() {
        val bridge = manifest().children("permission").single { it.getAttributeNS(android, "name") == "com.daanh.phonectl.permission.BRIDGE" }
        assertEquals("signature", bridge.getAttributeNS(android, "protectionLevel"))
    }

    @Test
    fun instrumentationTargetsOnlyThisApp() {
        val instrumentation = manifest().children("instrumentation")
        assertEquals(1, instrumentation.size)
        assertEquals("com.daanh.phonectl", instrumentation[0].getAttributeNS(android, "targetPackage"))
    }

    @Test
    fun watcherAdoptsOnlyListedPermissions() {
        val candidates = listOf("src/main/java/com/daanh/phonectl/ShellWatcher.kt", "app/src/main/java/com/daanh/phonectl/ShellWatcher.kt")
        val source = candidates.map(::File).first { it.exists() }.readText()
        val calls = Regex("""adoptShellPermissionIdentity\(([^)]*)\)""").findAll(source).map { it.groupValues[1] }.toList()
        assertTrue(calls.isNotEmpty())
        val allowed = setOf("READ_CLIPBOARD_IN_BACKGROUND", "INSTALL_PACKAGES")
        for (call in calls) {
            val args = call.split(",").map { it.trim() }
            // Never the no-argument form, which adopts every shell permission.
            assertTrue("adopt call without arguments", args.none { it.isEmpty() })
            assertTrue("unexpected permission in adopt($call)", args.all { it in allowed })
        }
        val constants = Regex("""const val (\w+) = "android\.permission\.(\w+)"""").findAll(source).map { it.groupValues[1] }.toSet()
        assertEquals(allowed, constants)
        assertTrue("UiAutomation must not be used for anything else",
            Regex("""\.(injectInputEvent|takeScreenshot|getWindows|executeShellCommand|setRotation|grantRuntimePermission|dropShellPermissionIdentity)""").find(source) == null)
        assertTrue("UiAutomation must stay private to ShellWatcher.kt",
            candidates.map(::File).first { it.exists() }.parentFile.listFiles()!!
                .filter { it.name != "ShellWatcher.kt" }.none { it.readText().contains("UiAutomation") })
    }
}
