package dev.verncat.player

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.io.File

class SelfUpdaterTest {
    @Test fun onlyOfficialUniversalReleaseAddressesAreAccepted() {
        SelfUpdatePolicy.officialUrl("https://github.com/NoName-Audio/player/releases/download/v0.3.8/player-android-universal.apk")
        for (url in listOf(
            "http://github.com/NoName-Audio/player/releases/download/v0.3.8/player-android-universal.apk",
            "https://evil.example/NoName-Audio/player/releases/download/v0.3.8/player-android-universal.apk",
            "https://github.com/Other/player/releases/download/v0.3.8/player-android-universal.apk",
            "https://github.com/NoName-Audio/player/releases/download/v0.3.8/arm64.apk",
            "https://github.com/NoName-Audio/player/releases/download/v0.3.8/player-android-universal.apk?redirect=evil"
        )) assertThrows(IllegalArgumentException::class.java) { SelfUpdatePolicy.officialUrl(url) }
    }

    @Test fun installedApkCannotBeReinstalledAsANewerUpdate() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val failure = assertThrows(IllegalArgumentException::class.java) {
            SelfUpdatePolicy.verify(context, File(context.applicationInfo.sourceDir))
        }
        assertTrue(failure.message.orEmpty().contains("newer"))
    }

    @Test fun realForeignSignedApkIsRefused() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val failure = assertThrows(IllegalArgumentException::class.java) {
            SelfUpdatePolicy.verify(instrumentation.targetContext, File(instrumentation.context.applicationInfo.sourceDir))
        }
        assertTrue(failure.message.orEmpty().contains("this application"))
    }

    @Test fun malformedArchiveIsRefusedBeforeInstaller() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File.createTempFile("invalid-update-", ".apk", context.cacheDir)
        try {
            file.writeText("not an APK")
            assertThrows(IllegalArgumentException::class.java) { SelfUpdatePolicy.verify(context, file) }
        } finally { file.delete() }
    }
    @Test fun signedFixturesEnforceCertificateAndDetectTampering() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val arguments = InstrumentationRegistry.getArguments()
        val compatible = File(requireNotNull(arguments.getString("compatibleApk")))
        val foreign = File(requireNotNull(arguments.getString("foreignApk")))
        assertEquals(context.packageName, SelfUpdatePolicy.verify(context, compatible).packageName)
        val refused = assertThrows(IllegalArgumentException::class.java) { SelfUpdatePolicy.verify(context, foreign) }
        assertTrue(refused.message.orEmpty().contains("certificate"))
        val tampered = File.createTempFile("tampered-", ".apk", context.cacheDir)
        try {
            compatible.copyTo(tampered, overwrite = true)
            java.io.RandomAccessFile(tampered, "rw").use { apk ->
                apk.seek(100)
                val byte = apk.readByte()
                apk.seek(100)
                apk.writeByte(byte.toInt() xor 1)
            }
            assertThrows(Exception::class.java) { SelfUpdatePolicy.verify(context, tampered) }
        } finally { tampered.delete() }
    }
    @Test fun permissionReturnLaunchesRealInstallerWithReadableApk() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val source = File(requireNotNull(InstrumentationRegistry.getArguments().getString("compatibleApk")))
        val activity = instrumentation.startActivitySync(android.content.Intent().setClassName(context, "dev.verncat.player.UpdaterTestActivity")
            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
        fun shell(command: String) {
            instrumentation.uiAutomation.executeShellCommand(command).use { descriptor ->
                java.io.FileInputStream(descriptor.fileDescriptor).use { it.readBytes() }
            }
        }
        fun field(name: String, value: Any?) {
            SelfUpdater::class.java.getDeclaredField(name).apply { isAccessible = true }.set(null, value)
        }
        val directory = File(context.cacheDir, "self-update").apply { mkdirs() }
        val apk = File(directory, "player-instrumented.apk")
        source.copyTo(apk, overwrite = true)
        val launches = mutableListOf<android.content.Intent>()
        val invoke = app.tauri.plugin.Invoke(1, "test", 1, 2, { _, _ -> }, "{}", com.fasterxml.jackson.databind.ObjectMapper())
        lateinit var updater: SelfUpdater
        try {
            instrumentation.runOnMainSync {
                updater = SelfUpdater(activity) { _, intent, _ ->
                    launches.add(intent)
                    activity.startActivity(intent)
                }
                field("file", apk); field("status", "permission"); field("permissionLaunched", false)
                ApkInstallGate.busy.set(true)
                updater.continueUpdate(invoke)
            }
            instrumentation.waitForIdleSync()
            assertEquals(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, launches.single().action)
            shell("appops set ${context.packageName} REQUEST_INSTALL_PACKAGES allow")
            instrumentation.runOnMainSync { updater.permissionResult(invoke) }
            instrumentation.waitForIdleSync()
            val intent = launches.last()
            assertEquals(android.content.Intent.ACTION_INSTALL_PACKAGE, intent.action)
            assertEquals("application/vnd.android.package-archive", intent.type)
            assertTrue(intent.flags and android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION != 0)
            assertEquals(intent.data, intent.clipData?.getItemAt(0)?.uri)
            context.contentResolver.openInputStream(requireNotNull(intent.data)).use { input ->
                assertArrayEquals(source.readBytes(), requireNotNull(input).readBytes())
            }
            instrumentation.runOnMainSync {
                updater.installResult(androidx.activity.result.ActivityResult(android.app.Activity.RESULT_CANCELED, null))
            }
            assertFalse(apk.exists())
            assertFalse(ApkInstallGate.busy.get())
        } finally {
            instrumentation.runOnMainSync { updater.close(); activity.finish() }
            apk.delete()
        }
    }
}
