package dev.verncat.player

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.result.ActivityResult
import androidx.core.content.FileProvider
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File
import java.net.URL
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.HttpsURLConnection

/** Downloads are private and never executed; only Android's package installer installs APKs. */
internal class CatalogApkInstaller(
    private val activity: Activity,
    private val runtime: InstalledApkRuntime,
    private val launch: (Invoke, Intent, String) -> Unit,
) {
    companion object {
        private val busy = AtomicBoolean(false)
        private val downloads = Executors.newSingleThreadExecutor()
        private const val LIMIT = 64L * 1024 * 1024
    }
    private class Pending(val invoke: Invoke) {
        var file: File? = null
        var archive: InstalledApkRuntime.Descriptor? = null
        var uri: Uri? = null
    }
    private var pending: Pending? = null
    private var closed = false

    @Synchronized fun install(invoke: Invoke) {
        if (closed || !busy.compareAndSet(false, true)) {
            invoke.resolve(outcome("failed", "Another APK installation is in progress"))
            return
        }
        val state = Pending(invoke)
        pending = state
        downloads.execute {
            try {
                val args = invoke.parseArgs(ApkInstallArgs::class.java)
                val directory = File(activity.cacheDir, "catalog-apks").apply { check(mkdirs() || isDirectory) }
                val file = File.createTempFile("catalog-", ".apk", directory)
                synchronized(this) {
                    if (pending !== state) { file.delete(); return@execute }
                    state.file = file
                }
                download(args.url, file)
                val archive = runtime.archive(file)
                require(org.json.JSONObject(archive.content).getJSONObject("manifest").getString("version") == args.expectedVersion) {
                    "Catalog version does not match plugin manifest"
                }
                require(archive.id == args.expectedId) { "Catalog plugin ID does not match APK" }
                require(archive.pkg.versionName == args.expectedVersion) { "Catalog version does not match APK" }
                // Do not permit signer rotation or replacement of a differently signed package.
                try {
                    @Suppress("DEPRECATION")
                    val existing = activity.packageManager.getPackageInfo(archive.pkg.packageName,
                        PackageManager.GET_SIGNATURES or if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else 0)
                    @Suppress("DEPRECATION")
                    val signers = if (Build.VERSION.SDK_INT >= 28) existing.signingInfo?.apkContentsSigners else existing.signatures
                    @Suppress("DEPRECATION")
                    val incoming = if (Build.VERSION.SDK_INT >= 28) archive.pkg.signingInfo?.apkContentsSigners else archive.pkg.signatures
                    require(signers != null && incoming != null && signers.size == 1 && incoming.size == 1 && signers[0] == incoming[0]) {
                        "Installed package has a different signing certificate"
                    }
                } catch (_: PackageManager.NameNotFoundException) { /* New package. */ }
                activity.runOnUiThread {
                    synchronized(this) {
                        if (pending !== state) return@runOnUiThread
                        state.archive = archive
                        try {
                            if (Build.VERSION.SDK_INT >= 26 && !activity.packageManager.canRequestPackageInstalls()) {
                                launch(invoke, Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                    Uri.parse("package:${activity.packageName}")), "catalogPermissionResult")
                            } else launchInstaller(state)
                        } catch (error: Exception) { finish(state, "failed", error.message) }
                    }
                }
            } catch (error: Exception) { finish(state, "failed", error.message) }
            finally { synchronized(this) { if (pending !== state) state.file?.delete() } }
        }
    }

    @Synchronized fun permissionResult(invoke: Invoke) {
        val state = pending?.takeIf { it.invoke === invoke } ?: return
        if (Build.VERSION.SDK_INT >= 26 && !activity.packageManager.canRequestPackageInstalls()) {
            finish(state, "permission-denied", "Permission to install APKs was not granted")
            return
        }
        try { launchInstaller(state) } catch (error: Exception) { finish(state, "failed", error.message) }
    }

    @Suppress("DEPRECATION")
    private fun launchInstaller(state: Pending) {
        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.catalog-apks", requireNotNull(state.file))
        state.uri = uri
        val intent = Intent(Intent.ACTION_INSTALL_PACKAGE).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            clipData = ClipData.newRawUri("Plugin APK", uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            putExtra(Intent.EXTRA_RETURN_RESULT, true)
        }
        launch(state.invoke, intent, "catalogInstallResult")
    }

    @Synchronized fun installResult(invoke: Invoke, result: ActivityResult) {
        val state = pending?.takeIf { it.invoke === invoke } ?: return
        if (result.resultCode != Activity.RESULT_OK) {
            finish(state, if (result.resultCode == Activity.RESULT_CANCELED) "cancelled" else "failed",
                if (result.resultCode == Activity.RESULT_CANCELED) "APK installation cancelled" else "Android could not install the APK")
            return
        }
        downloads.execute {
            try {
                val expected = requireNotNull(state.archive)
                val installed = runtime.descriptor(expected.pkg.packageName)
                require(installed.id == expected.id && installed.pkg.versionName == expected.pkg.versionName &&
                    installed.hash == expected.hash && installed.certificate == expected.certificate) {
                    "Installed APK does not match the reviewed download"
                }
                val discovered = activity.packageManager.queryIntentActivities(Intent("dev.verncat.player.PLUGIN"), 0)
                require(discovered.any { it.activityInfo.packageName == expected.pkg.packageName }) { "Installed APK is not a discoverable plugin" }
                finish(state, "installed", content = installed.content)
            } catch (error: Exception) { finish(state, "failed", error.message) }
        }
    }

    @Synchronized private fun finish(state: Pending, status: String, message: String? = null, content: String? = null) {
        if (pending !== state) return
        pending = null
        state.uri?.let { activity.revokeUriPermission(it, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        state.file?.delete()
        busy.set(false)
        state.invoke.resolve(outcome(status, message, content))
    }

    @Synchronized fun close() {
        closed = true
        pending?.let { finish(it, "cancelled", "Activity closed during APK installation") }
    }

    private fun outcome(status: String, message: String? = null, content: String? = null) = JSObject().apply {
        put("status", status)
        message?.let { put("message", it.take(2000)) }
        content?.let { put("content", it) }
    }

    private fun download(address: String, file: File) {
        var url = URL(address)
        val deadline = android.os.SystemClock.elapsedRealtime() + 120_000
        repeat(6) { redirect ->
            require(url.protocol == "https" && url.userInfo == null) { "APK downloads require HTTPS without credentials" }
            val connection = url.openConnection() as HttpsURLConnection
            try {
                connection.instanceFollowRedirects = false
                connection.connectTimeout = 15_000
                connection.readTimeout = 15_000
                val code = connection.responseCode
                if (code in listOf(301, 302, 303, 307, 308)) {
                    require(redirect < 5) { "Too many APK download redirects" }
                    url = URL(url, requireNotNull(connection.getHeaderField("Location")))
                } else {
                    require(code == 200) { "APK download failed (HTTP $code)" }
                    require(connection.contentLengthLong <= LIMIT) { "APK exceeds 64 MiB" }
                    connection.inputStream.use { input ->
                        file.outputStream().use { output ->
                            val buffer = ByteArray(65536)
                            var total = 0L
                            while (true) {
                                check(android.os.SystemClock.elapsedRealtime() < deadline) { "APK download timed out" }
                                synchronized(this) { check(!closed && pending != null) { "APK download cancelled" } }
                                val count = input.read(buffer)
                                if (count < 0) break
                                total += count
                                require(total <= LIMIT) { "APK exceeds 64 MiB" }
                                output.write(buffer, 0, count)
                            }
                            require(total > 0) { "Empty APK download" }
                        }
                    }
                    return
                }
            } finally { connection.disconnect() }
        }
        error("Too many APK download redirects")
    }
}
