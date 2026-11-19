package dev.verncat.player

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Process
import android.provider.OpenableColumns
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.URI
import java.util.ArrayDeque
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

@TauriPlugin
class ShareInboxPlugin(activity: Activity) : Plugin(activity) {
    @Command
    fun awaitItems(invoke: Invoke) = ShareInbox.await(this, invoke)

    @Command
    fun chooseHandler(invoke: Invoke) {
        val choices = invoke.getArgs().getJSONArray("choices")
        val labels = Array(choices.length()) { choices.getString(it) }
        fun resolve(index: Int) = invoke.resolve(JSObject().apply { put("index", index) })
        val activity = MainActivity.instance?.get() ?: return resolve(-1)
        activity.runOnUiThread {
            if (activity.isFinishing || activity.isDestroyed) {
                resolve(-1)
                return@runOnUiThread
            }
            var selected = -1
            val dialog = AlertDialog.Builder(activity)
                .setTitle("Open with")
                .setItems(labels) { _, index -> selected = index }
                .setNegativeButton(android.R.string.cancel, null)
                .create()
            dialog.setOnDismissListener { resolve(selected) }
            activity.showShareDialog(dialog)
        }
    }

    override fun onDestroy() {
        ShareInbox.detach(this)
        super.onDestroy()
    }
}

/** One consumer and one FIFO batch per intent, independent of when the WebView starts. */
internal object ShareInbox {
    private const val MAX_ITEMS = 32
    private const val MAX_BATCHES = 16
    private const val FILE_LIMIT = 128L * 1024 * 1024
    private const val CACHE_LIMIT = 512L * 1024 * 1024
    private const val MAX_FILES = 512
    private const val MAX_TEXT = 8192
    private val links = Regex("https?://[^\\s<>\"']+", RegexOption.IGNORE_CASE)
    private val unsafeName = Regex("[\\p{Cc}\\p{Cf}/\\\\:*?\"<>|]")
    private val worker = ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS,
        ArrayBlockingQueue(MAX_BATCHES), ThreadPoolExecutor.DiscardPolicy())
    private val ready = ArrayDeque<JSONArray>()
    private var waiter: Pair<ShareInboxPlugin, Invoke>? = null

    @Synchronized
    fun await(owner: ShareInboxPlugin, invoke: Invoke) {
        val items = ready.pollFirst()
        when {
            items != null -> resolve(invoke, items)
            waiter != null -> invoke.reject("A share inbox request is already pending")
            else -> waiter = owner to invoke
        }
    }

    @Synchronized
    fun detach(owner: ShareInboxPlugin) {
        if (waiter?.first === owner) waiter = null
    }

    private fun resolve(invoke: Invoke, items: JSONArray) {
        invoke.resolve(JSObject().apply { put("items", items) })
    }

    @Synchronized
    private fun publish(items: JSONArray) {
        val pending = waiter?.second
        if (pending == null) ready.addLast(items) else {
            waiter = null
            resolve(pending, items)
        }
    }

    fun receive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_SEND && intent.action != Intent.ACTION_SEND_MULTIPLE) return
        worker.execute {
            // This single producer cannot race another producer for the remaining slot.
            if (synchronized(this) { ready.size >= MAX_BATCHES }) return@execute
            try {
                collect(context, intent).let { if (it.length() != 0) publish(it) }
            } catch (_: RuntimeException) {
                // Extras from an exported activity may not be valid parcelables.
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun collect(context: Context, intent: Intent): JSONArray {
        val result = JSONArray()
        val seen = HashSet<String>()
        fun addText(text: CharSequence?) {
            if (text == null || text.length > MAX_TEXT) return
            for (match in links.findAll(text)) {
                if (result.length() == MAX_ITEMS) break
                val value = match.value
                val parsed = try { URI(value) } catch (_: java.net.URISyntaxException) { continue }
                if (parsed.host != null && parsed.rawUserInfo == null && seen.add(value)) {
                    result.put(JSONObject().put("url", value))
                }
            }
        }
        fun addUri(uri: Uri?) {
            if (uri == null || result.length() == MAX_ITEMS) return
            val key = uri.toString()
            if (key.length > MAX_TEXT) return
            if (uri.scheme.equals("http", true) || uri.scheme.equals("https", true)) addText(key)
            else if (uri.scheme == "content" && seen.add(key)) {
                try { stage(context, uri, intent.type)?.let { result.put(it) } } catch (_: Exception) {
                    // A revoked grant or unavailable provider should not discard the other items.
                }
            }
        }
        addText(intent.getCharSequenceExtra(Intent.EXTRA_TEXT))
        val streams = intent.extras?.get(Intent.EXTRA_STREAM)
        when (streams) {
            is Uri -> addUri(streams)
            is List<*> -> for (i in 0 until minOf(streams.size, MAX_ITEMS)) addUri(streams[i] as? Uri)
        }
        intent.clipData?.let { clip ->
            for (i in 0 until minOf(clip.itemCount, MAX_ITEMS)) {
                val item = clip.getItemAt(i)
                addText(item.text)
                addUri(item.uri)
            }
        }
        return result
    }

    private fun stage(context: Context, uri: Uri, fallbackMime: String?): JSONObject? {
        val authority = uri.authority ?: return null
        if ('@' in authority || authority.isBlank()) return null
        // Do not lend the sender the app's own storage/provider privileges.
        if (context.checkUriPermission(uri, Process.myPid(), Process.myUid(),
                Intent.FLAG_GRANT_READ_URI_PERMISSION) != PackageManager.PERMISSION_GRANTED) return null
        if (context.packageManager.resolveContentProvider(authority, 0)?.applicationInfo?.uid == Process.myUid()) return null

        val dir = File(context.cacheDir, "share-inbox")
        if (!dir.isDirectory && !dir.mkdir()) return null
        val files = dir.listFiles() ?: return null
        if (files.size >= MAX_FILES) return null
        val limit = minOf(FILE_LIMIT, CACHE_LIMIT - files.sumOf { it.length() })
        if (limit <= 0) return null
        val resolver = context.contentResolver
        val display = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }
        val name = (display ?: uri.lastPathSegment.orEmpty()).take(160).replace(unsafeName, "_")
            .trim().trim('.').ifBlank { "shared-file" }
        val mime = (resolver.getType(uri) ?: fallbackMime)?.take(255) ?: "application/octet-stream"
        val target = File.createTempFile("share-", null, dir)
        var complete = false
        try {
            resolver.openInputStream(uri)?.use { input ->
                target.outputStream().use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var size = 0L
                    while (true) {
                        val count = input.read(buffer)
                        if (count < 0) break
                        size += count
                        if (count == 0 || size > limit) return null
                        output.write(buffer, 0, count)
                    }
                }
            } ?: return null
            val item = JSONObject().put("name", name).put("mime", mime).put("path", target.absolutePath)
            complete = true
            return item
        } finally {
            if (!complete) target.delete()
        }
    }
}
