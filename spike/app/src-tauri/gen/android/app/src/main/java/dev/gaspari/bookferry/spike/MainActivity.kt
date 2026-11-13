package dev.gaspari.bookferry.spike

import android.app.Activity
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.ConcurrentHashMap

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private val pickedUris = ConcurrentHashMap<String, Uri>()

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    handleCommandIntent(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    handleCommandIntent(intent)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    appWebView = webView
    webView.addJavascriptInterface(SafBridge(), "AndroidSaf")
    Log.i("BookferrySpike", "WebView created, UA=${webView.settings.userAgentString}")
  }

  private fun handleCommandIntent(intent: Intent?) {
    if (intent == null) return
    val evalJs = intent.getStringExtra("eval")
    if (!evalJs.isNullOrEmpty()) {
      Log.i("BookferrySpike", "Executing intent eval: $evalJs")
      appWebView?.post {
        appWebView?.evaluateJavascript(evalJs, null)
      }
    }
    val pickSaf = intent.getStringExtra("pick_saf")
    if (!pickSaf.isNullOrEmpty()) {
      val allowMultiple = pickSaf == "multi"
      launchSafPicker(allowMultiple)
    }
  }

  private fun queryUriMeta(uri: Uri): Pair<String, Long> {
    var displayName = uri.lastPathSegment ?: "unknown"
    var size = -1L
    val cursor: Cursor? = contentResolver.query(uri, null, null, null, null)
    cursor?.use {
      if (it.moveToFirst()) {
        val nameIdx = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (nameIdx >= 0 && !it.isNull(nameIdx)) {
          displayName = it.getString(nameIdx)
        }
        val sizeIdx = it.getColumnIndex(OpenableColumns.SIZE)
        if (sizeIdx >= 0 && !it.isNull(sizeIdx)) {
          size = it.getLong(sizeIdx)
        }
      }
    }
    return Pair(displayName, size)
  }

  private fun detachUriFd(uri: Uri): JSONObject {
    val t0 = SystemClock.elapsedRealtimeNanos()
    val (displayName, metaSize) = queryUriMeta(uri)
    val pfd = contentResolver.openFileDescriptor(uri, "r")
      ?: throw IllegalStateException("openFileDescriptor returned null for $uri")
    val statSize = pfd.statSize
    val fd = pfd.detachFd()
    val openFdMs = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
    pickedUris[displayName] = uri
    pickedUris[uri.toString()] = uri
    return JSONObject().apply {
      put("uri", uri.toString())
      put("name", displayName)
      put("size", if (statSize >= 0) statSize else metaSize)
      put("fd", fd)
      put("openFdMs", openFdMs)
    }
  }

  fun launchSafPicker(allowMultiple: Boolean) {
    val initialUri = Uri.parse("content://com.android.externalstorage.documents/document/primary%3ADownload%2Fbookferry-fixtures")
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "*/*"
      putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple)
      putExtra(DocumentsContract.EXTRA_INITIAL_URI, initialUri)
    }
    Log.i("BookferrySpike", "Launching ACTION_OPEN_DOCUMENT (allowMultiple=$allowMultiple)")
    launchActivityForResult(intent) { result ->
      try {
        if (result == null || result.resultCode != Activity.RESULT_OK) {
          Log.w("BookferrySpike", "SAF picker cancelled or failed: resultCode=${result?.resultCode}")
          return@launchActivityForResult
        }
        val data = result.data
        val items = JSONArray()
        val clipData = data?.clipData
        if (clipData != null && clipData.itemCount > 0) {
          for (i in 0 until clipData.itemCount) {
            val u = clipData.getItemAt(i).uri
            try {
              contentResolver.takePersistableUriPermission(u, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            } catch (_: Exception) {}
            items.put(detachUriFd(u))
          }
        } else {
          val u = data?.data
          if (u != null) {
            try {
              contentResolver.takePersistableUriPermission(u, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            } catch (_: Exception) {}
            items.put(detachUriFd(u))
          }
        }
        Log.i("BookferrySpike", "SAF_PICKED_JSON: $items")
        val jsPayload = items.toString()
        appWebView?.post {
          appWebView?.evaluateJavascript("window.__onSafPicked && window.__onSafPicked($jsPayload)", null)
        }
      } catch (e: Exception) {
        Log.e("BookferrySpike", "Error handling SAF result", e)
      }
    }
  }

  inner class SafBridge {
    @JavascriptInterface
    fun pickDocument(allowMultiple: Boolean) {
      runOnUiThread {
        launchSafPicker(allowMultiple)
      }
    }

    @JavascriptInterface
    fun reopenFd(keyOrUri: String): String {
      val uri = pickedUris[keyOrUri] ?: Uri.parse(keyOrUri)
      return detachUriFd(uri).toString()
    }

    @JavascriptInterface
    fun log(msg: String) {
      val chunkSize = 900
      if (msg.length <= chunkSize) {
        Log.i("BookferrySpike", msg)
      } else {
        var i = 0
        var part = 1
        val total = (msg.length + chunkSize - 1) / chunkSize
        while (i < msg.length) {
          val end = (i + chunkSize).coerceAtMost(msg.length)
          Log.i("BookferrySpike", "[part $part/$total] " + msg.substring(i, end))
          i = end
          part++
        }
      }
    }
  }
}
