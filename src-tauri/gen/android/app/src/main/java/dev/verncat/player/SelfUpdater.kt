package dev.verncat.player

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.result.ActivityResult
import androidx.core.content.FileProvider
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.HttpsURLConnection

/** Shared across both APK installation entry points, including activity recreation. */
internal object ApkInstallGate { val busy = AtomicBoolean(false) }

@InvokeArg
internal class SelfUpdateArgs { lateinit var url: String }

/** The OS verifies APK cryptographic signatures; archive parsing additionally binds the download
 * to this installed application's identity, newer version and current signing certificates. */
internal object SelfUpdatePolicy {
    fun officialUrl(address: String) {
        val uri = Uri.parse(address)
        require(uri.scheme == "https" && uri.host == "github.com" && uri.port == -1 &&
            uri.userInfo == null && uri.query == null && uri.fragment == null &&
            Regex("/NoName-Audio/player/releases/download/[^/]+/player-android-universal\\.apk").matches(uri.encodedPath ?: "")) {
            "Only the official Player universal release APK can be downloaded"
        }
    }

    @Suppress("DEPRECATION")
    fun verify(context: android.content.Context, file: File): PackageInfo {
        val flags = PackageManager.GET_SIGNATURES or
            if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else 0
        val incoming = requireNotNull(context.packageManager.getPackageArchiveInfo(file.absolutePath, flags)) { "Invalid or unsigned APK" }
        val installed = context.packageManager.getPackageInfo(context.packageName, flags)
        require(incoming.packageName == context.packageName) { "APK is not an update for this application" }
        fun version(pkg: PackageInfo): Long = if (Build.VERSION.SDK_INT >= 28) pkg.longVersionCode else pkg.versionCode.toLong()
        require(version(incoming) > version(installed)) { "APK version must be newer than the installed application" }
        val app = requireNotNull(incoming.applicationInfo) { "APK has no application" }
        require(app.splitSourceDirs.isNullOrEmpty() && incoming.splitNames.isNullOrEmpty()) { "A complete universal APK is required" }
        fun signers(pkg: PackageInfo) = if (Build.VERSION.SDK_INT >= 28) pkg.signingInfo?.apkContentsSigners else pkg.signatures
        val current = requireNotNull(signers(installed)) { "Installed signing certificate unavailable" }
        val next = requireNotNull(signers(incoming)) { "APK signing certificate unavailable" }
        // Deliberately conservative: exact current signer set. Do not accept unverified lineage or rotation.
        require(current.isNotEmpty() && next.size == current.size && current.toSet() == next.toSet()) {
            "APK signing certificate differs from the installed application"
        }
        val verified = com.android.apksig.ApkVerifier.Builder(file)
            .setMinCheckedPlatformVersion(Build.VERSION.SDK_INT).build().verify()
        require(verified.isVerified) { "APK cryptographic signature verification failed" }
        val verifiedCertificates = verified.signerCertificates.map { it.encoded.toList() }.toSet()
        require(verifiedCertificates == current.map { it.toByteArray().toList() }.toSet()) {
            "Verified APK signing certificate differs from the installed application"
        }
        return incoming
    }
}

internal class SelfUpdater(private val activity: Activity, private val launch: (Invoke, Intent, String) -> Unit) {
    companion object {
        private val worker = Executors.newSingleThreadExecutor()
        private val lock = Any()
        private var status = "idle"
        private var downloaded = 0L
        private var total = 0L
        private var error: String? = null
        private var file: File? = null
        private var uri: Uri? = null
        private var generation = 0L
        private var owner: SelfUpdater? = null
        private var permissionLaunched = false
        private const val LIMIT = 1024L * 1024 * 1024
    }

    init { synchronized(lock) { owner = this } }

    fun snapshot(invoke: Invoke) {
        synchronized(lock) {
            if (status == "permission" && (!permissionLaunched || activity.packageManager.canRequestPackageInstalls())) advance(invoke)
            invoke.resolve(result())
        }
    }

    fun start(invoke: Invoke) {
        synchronized(lock) {
            var acquired = false
            try {
                check(ApkInstallGate.busy.compareAndSet(false, true)) { "Another APK installation is in progress" }
                acquired = true
                val address = invoke.parseArgs(SelfUpdateArgs::class.java).url
                SelfUpdatePolicy.officialUrl(address)
                val directory = File(activity.cacheDir, "self-update").apply { check(mkdirs() || isDirectory) }
                // Only updater-owned leftovers, from a previous process, are removed.
                directory.listFiles()?.filter { it.name.startsWith("player-") }?.forEach { it.delete() }
                file = File.createTempFile("player-", ".part", directory)
                status = "downloading"; downloaded = 0; total = 0; error = null
                permissionLaunched = false
                val token = ++generation
                val destination = requireNotNull(file)
                val context = activity.applicationContext
                worker.execute {
                    try {
                        download(address, destination, token)
                        synchronized(lock) { check(generation == token); status = "verifying" }
                        SelfUpdatePolicy.verify(context, destination)
                        synchronized(lock) {
                            check(generation == token)
                            val complete = File(directory, "player-$token.apk")
                            check(destination.renameTo(complete)) { "Could not finalize APK download" }
                            file = complete
                            status = "permission"
                            owner?.activity?.runOnUiThread { synchronized(lock) { if (generation == token) owner?.advance(invoke) } }
                        }
                    } catch (failure: Exception) {
                        synchronized(lock) { if (generation == token) finish("error", failure.message) }
                    } finally { if (destination.extension == "part") destination.delete() }
                }
                invoke.resolve(result())
            } catch (failure: Exception) {
                // A rejected concurrent request must never release another installation's gate.
                if (acquired) finish("error", failure.message)
                invoke.reject(failure.message ?: "Unable to start update")
            }
        }
    }

    fun continueUpdate(invoke: Invoke) { synchronized(lock) { advance(invoke); invoke.resolve(result()) } }

    private fun advance(invoke: Invoke) {
        if (owner !== this || activity.isFinishing || activity.isDestroyed || status != "permission") return
        try {
            if (!activity.packageManager.canRequestPackageInstalls()) {
                if (!permissionLaunched) {
                    permissionLaunched = true
                    launch(invoke, Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}")), "selfUpdatePermissionResult")
                }
                return
            }
            val apk = requireNotNull(file)
            val content = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", apk)
            uri = content
            val intent = Intent(Intent.ACTION_INSTALL_PACKAGE).apply {
                setDataAndType(content, "application/vnd.android.package-archive")
                clipData = ClipData.newRawUri("Player update", content)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                putExtra(Intent.EXTRA_RETURN_RESULT, true)
            }
            status = "installing"
            launch(invoke, intent, "selfUpdateInstallResult")
        } catch (failure: Exception) { finish("error", failure.message) }
    }

    fun permissionResult(invoke: Invoke) { synchronized(lock) {
        if (status != "permission") return
        permissionLaunched = false
        if (!activity.packageManager.canRequestPackageInstalls()) finish("error", "Allow installation from Player in Android settings, then try again")
        else advance(invoke)
    } }

    fun installResult(result: ActivityResult) { synchronized(lock) {
        if (status != "installing") return
        // Replacing Player normally kills this process. A surviving callback is not proof of installation.
        finish(if (result.resultCode == Activity.RESULT_CANCELED) "cancelled" else "error",
            if (result.resultCode == Activity.RESULT_CANCELED) "Android installation cancelled" else "Installer returned without replacing Player. Reopen Player and check its version")
    } }

    fun cancel(invoke: Invoke) { synchronized(lock) {
        if (status == "installing") invoke.reject("Finish or cancel installation in Android's installer")
        else { if (status in setOf("downloading", "verifying", "permission")) finish("cancelled", "Update cancelled"); invoke.resolve(result()) }
    } }

    fun close() { synchronized(lock) {
        if (owner !== this) return
        owner = null
        // Downloads survive activity recreation. Never revoke a URI while Android is reading it.
        if (status == "permission") permissionLaunched = false
    } }

    private fun finish(next: String, message: String?) {
        generation++
        uri?.let { activity.revokeUriPermission(it, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        uri = null
        file?.delete(); file = null
        status = next; error = message?.take(2000)
        ApkInstallGate.busy.set(false)
    }

    private fun result() = JSObject().apply {
        put("status", status); put("downloaded", downloaded); put("total", total)
        error?.let { put("error", it) }
    }

    private fun download(address: String, destination: File, token: Long) {
        var url = URL(address)
        val deadline = android.os.SystemClock.elapsedRealtime() + 30 * 60_000L
        repeat(6) { redirect ->
            require(url.protocol == "https" && url.userInfo == null) { "APK redirects must use HTTPS" }
            val connection = url.openConnection() as HttpsURLConnection
            try {
                connection.instanceFollowRedirects = false
                connection.connectTimeout = 20_000; connection.readTimeout = 30_000
                val code = connection.responseCode
                if (code in listOf(301, 302, 303, 307, 308)) {
                    require(redirect < 5) { "Too many download redirects" }
                    url = URL(url, requireNotNull(connection.getHeaderField("Location")))
                } else {
                    require(code == 200) { "APK download failed (HTTP $code)" }
                    val expected = connection.contentLengthLong
                    require(expected <= LIMIT) { "APK exceeds 1 GiB" }
                    synchronized(lock) { check(generation == token); total = expected.coerceAtLeast(0) }
                    connection.inputStream.use { input -> destination.outputStream().use { output ->
                        val buffer = ByteArray(65536)
                        var count = 0L
                        while (true) {
                            synchronized(lock) { check(generation == token) { "Update cancelled" } }
                            check(android.os.SystemClock.elapsedRealtime() < deadline) { "APK download timed out" }
                            val size = input.read(buffer)
                            if (size < 0) break
                            count += size
                            require(count <= LIMIT) { "APK exceeds 1 GiB" }
                            output.write(buffer, 0, size)
                            synchronized(lock) { if (generation == token) downloaded = count }
                        }
                        require(count > 0 && (expected < 0 || expected == count)) { "APK download is incomplete" }
                        output.fd.sync()
                    } }
                    return
                }
            } finally { connection.disconnect() }
        }
        error("Too many download redirects")
    }
}
