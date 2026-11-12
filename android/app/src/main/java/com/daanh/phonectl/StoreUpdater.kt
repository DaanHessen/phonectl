package com.daanh.phonectl

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.Executors

/**
 * The owner's own apps, the only ones phonectl will ever install. An APK must
 * have one of these package names, be signed only by that package's pinned
 * certificate and have a higher version than the installed app (no
 * downgrades, no new apps).
 */
object OwnApps {
    const val PHONECTL = "com.daanh.phonectl"
    private const val LAPTOP_KEY = "32bae3c60bcaa36d06e2489c76b43f831ee14949da107b6d3945acb6b93fb63c"
    private const val NOT_BAD_COMPANY_KEY = "37acbbf0dc50e469cee1f96eab169b74ab3e4eda966df6af4b429674a8be3999"

    /**
     * Package name to the SHA-256 of its one allowed signing certificate
     * (`apksigner verify --print-certs`). Iteration order is install order:
     * phonectl last, because updating it restarts this process.
     */
    val PINS: Map<String, String> = linkedMapOf(
        "org.daanh.vector_phone" to LAPTOP_KEY,
        "org.daanh.crew" to LAPTOP_KEY,
        "com.daanh.krentenweger" to NOT_BAD_COMPANY_KEY,
        "com.daanh.phonectl.keyboard" to LAPTOP_KEY,
        PHONECTL to LAPTOP_KEY,
    )
    val PACKAGES: List<String> get() = PINS.keys.toList()

    fun installedVersion(context: Context, pkg: String): Long? = try {
        context.packageManager.getPackageInfo(pkg, 0).longVersionCode
    } catch (_: PackageManager.NameNotFoundException) {
        null
    }

    /** Checks an APK file; throws SecurityException with the reason. */
    fun verify(context: Context, apk: File, onlyPackage: String? = null): PackageInfo {
        val info = context.packageManager.getPackageArchiveInfo(apk.path, PackageManager.GET_SIGNING_CERTIFICATES)
            ?: throw SecurityException("not a valid APK")
        val pkg = info.packageName
        val pin = PINS[pkg]
        if (pin == null || (onlyPackage != null && pkg != onlyPackage)) throw SecurityException("$pkg is not one of the owner's apps")
        val signers = info.signingInfo?.apkContentsSigners ?: throw SecurityException("unsigned")
        if (signers.isEmpty()) throw SecurityException("unsigned")
        for (signer in signers) {
            val digest = MessageDigest.getInstance("SHA-256").digest(signer.toByteArray()).joinToString("") { "%02x".format(it) }
            if (digest != pin) throw SecurityException("$pkg is signed with an unknown certificate")
        }
        val installed = installedVersion(context, pkg) ?: throw SecurityException("$pkg is not installed (updates only)")
        if (info.longVersionCode <= installed) throw SecurityException("$pkg ${info.longVersionCode} is not newer than $installed")
        return info
    }
}

/**
 * Silent updates of the owner's own apps from the owner's phone store (the
 * Pi, over Tailscale). With the watcher running, phonectl borrows the shell's
 * INSTALL_PACKAGES for the few seconds of an install, so no prompt appears;
 * without it, nothing is installed (Obtainium still offers the update).
 */
object StoreUpdater : Link.Feature {
    private const val STORE = "http://100.66.214.122:8766/apps/"
    private const val MAX_APK_BYTES = 300L * 1024 * 1024
    private const val MIN_GAP_MS = 10 * 60_000L
    private const val CONNECT_GAP_MS = 60 * 60_000L

    private lateinit var context: Context
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "store-updater") }
    @Volatile private var lastCheck = 0L

    fun init(ctx: Context) {
        context = ctx.applicationContext
        Link.register(this)
    }

    override fun onConnected(session: Session) {
        if (System.currentTimeMillis() - lastCheck > CONNECT_GAP_MS) check("connected")
    }

    override fun onCall(method: String, params: JSONObject?): Any? {
        if (method != "store_check") return null
        return JSONObject().put("started", check("laptop"))
    }

    private const val PREFS = "store"
    private const val KEY_ENABLED = "silent_updates"

    /** Switched only in the app on the phone; the laptop cannot change it. */
    var enabled: Boolean
        get() = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ENABLED, true)
        set(value) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ENABLED, value).apply()
        }

    fun check(reason: String): Boolean {
        if (!enabled) return false
        val now = System.currentTimeMillis()
        if (now - lastCheck < MIN_GAP_MS) return false
        lastCheck = now
        worker.execute { safely { run(reason) } }
        return true
    }

    private fun safely(block: () -> Unit) = try { block() } catch (e: Exception) {
        Diag.log("store check failed: ${e.javaClass.simpleName}")
    }

    private fun run(reason: String) {
        for (pkg in OwnApps.PACKAGES) {
            val installed = OwnApps.installedVersion(context, pkg) ?: continue
            val newest = newestInStore(pkg) ?: continue
            if (newest.second <= installed) continue
            Diag.log("store: $pkg ${newest.second} is newer than $installed ($reason)")
            if (!enabled) return
            if (!ShellClip.active) {
                Diag.log("store: not installing $pkg, the watcher is not running")
                continue
            }
            val file = download(pkg, newest.first) ?: continue
            try {
                OwnApps.verify(context, file, pkg)
            } catch (e: SecurityException) {
                Diag.log("store: refused $pkg: ${e.message}")
                Actions.note("store_install", "update $pkg from the phone store", "refused: ${e.message}")
                file.delete()
                continue
            }
            install(pkg, file)
            return // one install per check; the next check picks up the rest
        }
    }

    private val linkPattern = Regex("""href="([^"/?#]+\.apk)"""")

    /** (file name, version code) of the newest APK listed for [pkg]. */
    private fun newestInStore(pkg: String): Pair<String, Long>? {
        val page = fetch(URL("$STORE$pkg/"), 512 * 1024)?.toString(Charsets.UTF_8) ?: return null
        val name = Regex("""^${Regex.escape(pkg)}-[A-Za-z0-9.+_]+-(\d{1,18})\.apk$""")
        return linkPattern.findAll(page)
            .mapNotNull { m -> name.find(m.groupValues[1])?.let { m.groupValues[1] to it.groupValues[1].toLong() } }
            .maxByOrNull { it.second }
    }

    private fun download(pkg: String, fileName: String): File? {
        val dir = File(context.cacheDir, "store").apply { mkdirs() }
        dir.listFiles()?.forEach { it.delete() }
        val file = File(dir, "$pkg.apk")
        val connection = URL("$STORE$pkg/$fileName").openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = false
        connection.connectTimeout = 15_000
        connection.readTimeout = 30_000
        return try {
            if (connection.responseCode != 200) return null
            connection.inputStream.use { input ->
                file.outputStream().use { out ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        total += n
                        if (total > MAX_APK_BYTES) throw SecurityException("APK too large")
                        out.write(buffer, 0, n)
                    }
                }
            }
            file
        } catch (e: Exception) {
            Diag.log("store: download of $pkg failed: ${e.javaClass.simpleName}")
            file.delete()
            null
        } finally {
            connection.disconnect()
        }
    }

    private fun fetch(url: URL, limit: Int): ByteArray? {
        val connection = url.openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = false
        connection.connectTimeout = 10_000
        connection.readTimeout = 15_000
        return try {
            if (connection.responseCode != 200) null
            else connection.inputStream.use { it.readNBytes(limit) }
        } catch (_: Exception) {
            null
        } finally {
            connection.disconnect()
        }
    }

    private fun install(pkg: String, file: File) {
        if (!ShellClip.borrowInstallPermission()) return
        try {
            val installer = context.packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
                setAppPackageName(pkg)
                setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
            val id = installer.createSession(params)
            installer.openSession(id).use { session ->
                file.inputStream().use { input ->
                    session.openWrite("base.apk", 0, file.length()).use { out ->
                        input.copyTo(out)
                        session.fsync(out)
                    }
                }
                val intent = Intent(context, UpdateReceiver::class.java).putExtra(UpdateReceiver.EXTRA_STORE, pkg)
                val pending = PendingIntent.getBroadcast(context, id, intent, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
                session.commit(pending.intentSender)
            }
            Diag.log("store: installing $pkg")
        } catch (e: Exception) {
            ShellClip.returnInstallPermission("install failed")
            Diag.log("store: install of $pkg failed: ${e.javaClass.simpleName}")
        } finally {
            file.delete()
        }
    }
}
