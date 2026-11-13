package dev.gaspari.bookferry.spike

import android.app.Activity
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class MainActivity : TauriActivity() {
  private var appWebView: WebView? = null
  private val pickedUris = ConcurrentHashMap<String, Uri>()
  private var multicastLock: WifiManager.MulticastLock? = null
  private var pendingEvalJs: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
      setShowWhenLocked(true)
      setTurnScreenOn(true)
    }
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    ensureMulticastLock(true)
    handleCommandIntent(intent)
  }

  override fun onDestroy() {
    try {
      if (multicastLock?.isHeld == true) {
        multicastLock?.release()
        Log.i("BookferrySpike", "MulticastLock released onDestroy")
      }
    } catch (_: Exception) {}
    super.onDestroy()
  }

  private fun ensureMulticastLock(acquire: Boolean): Boolean {
    return try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      if (multicastLock == null) {
        multicastLock = wifi.createMulticastLock("bookferry-mdns-lock").apply {
          setReferenceCounted(false)
        }
      }
      if (acquire) {
        if (multicastLock?.isHeld != true) {
          multicastLock?.acquire()
        }
      } else {
        if (multicastLock?.isHeld == true) {
          multicastLock?.release()
        }
      }
      val held = multicastLock?.isHeld == true
      Log.i("BookferrySpike", "MulticastLock state: acquire=$acquire isHeld=$held")
      held
    } catch (e: Exception) {
      Log.e("BookferrySpike", "MulticastLock error", e)
      false
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    handleCommandIntent(intent)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    appWebView = webView
    webView.addJavascriptInterface(SafBridge(), "AndroidSaf")
    Log.i("BookferrySpike", "WebView created, UA=${webView.settings.userAgentString}, multicastHeld=${multicastLock?.isHeld}")
    val pending = pendingEvalJs
    if (!pending.isNullOrEmpty()) {
      pendingEvalJs = null
      webView.postDelayed({
        Log.i("BookferrySpike", "Executing deferred onCreate eval: $pending")
        webView.evaluateJavascript(pending, null)
      }, 600)
    }
  }

  private fun handleCommandIntent(intent: Intent?) {
    if (intent == null) return
    val evalJs = intent.getStringExtra("eval")
    if (!evalJs.isNullOrEmpty()) {
      val wv = appWebView
      if (wv != null) {
        Log.i("BookferrySpike", "Executing intent eval immediately: $evalJs")
        wv.post {
          wv.evaluateJavascript(evalJs, null)
        }
      } else {
        Log.i("BookferrySpike", "Queueing intent eval until WebView is ready: $evalJs")
        pendingEvalJs = evalJs
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
    fun setMulticastLock(acquire: Boolean): Boolean {
      return ensureMulticastLock(acquire)
    }

    @JavascriptInterface
    fun isMulticastLockHeld(): Boolean {
      return multicastLock?.isHeld == true
    }

    @JavascriptInterface
    fun createWritableDownloadFd(fileName: String): String {
      // 1) Tenta criar diretamente em /sdcard/Download/bookferry-fixtures/<fileName>
      try {
        val dir = File("/sdcard/Download/bookferry-fixtures")
        if (!dir.exists()) dir.mkdirs()
        val f = File(dir, fileName)
        if (f.exists()) f.delete()
        val raf = RandomAccessFile(f, "rw")
        val pfd = ParcelFileDescriptor.dup(raf.fd)
        raf.close()
        val detached = pfd.detachFd()
        return JSONObject().apply {
          put("ok", true)
          put("fd", detached)
          put("path", f.absolutePath)
          put("method", "direct_posix_new_file")
        }.toString()
      } catch (e: Exception) {
        Log.w("BookferrySpike", "Direct POSIX create in Download/bookferry-fixtures failed, trying MediaStore: ${e.message}")
      }

      // 2) Tenta via MediaStore.Downloads em Download/bookferry-fixtures
      try {
        val resolver = applicationContext.contentResolver
        val collection = MediaStore.Downloads.EXTERNAL_CONTENT_URI
        // Remove entrada anterior do mesmo app se existir
        resolver.delete(
          collection,
          "${MediaStore.MediaColumns.DISPLAY_NAME}=?",
          arrayOf(fileName)
        )
        val values = ContentValues().apply {
          put(MediaStore.MediaColumns.DISPLAY_NAME, fileName)
          put(MediaStore.MediaColumns.MIME_TYPE, "application/vnd.comicbook+zip")
          put(MediaStore.MediaColumns.RELATIVE_PATH, "Download/bookferry-fixtures")
        }
        val uri = resolver.insert(collection, values)
          ?: throw IllegalStateException("MediaStore.insert returned null")
        val pfd = resolver.openFileDescriptor(uri, "rw")
          ?: throw IllegalStateException("openFileDescriptor(rw) returned null")
        val detached = pfd.detachFd()
        return JSONObject().apply {
          put("ok", true)
          put("fd", detached)
          put("path", "/sdcard/Download/bookferry-fixtures/$fileName")
          put("uri", uri.toString())
          put("method", "mediastore_downloads")
        }.toString()
      } catch (e: Exception) {
        Log.e("BookferrySpike", "MediaStore create failed: ${e.message}", e)
        return JSONObject().apply {
          put("ok", false)
          put("error", e.toString())
        }.toString()
      }
    }

    @JavascriptInterface
    fun discoverNsdAndroid(timeoutMs: Long): String {
      ensureMulticastLock(true)
      val t0 = SystemClock.elapsedRealtimeNanos()
      val nsdManager = applicationContext.getSystemService(Context.NSD_SERVICE) as NsdManager
      val latch = CountDownLatch(1)
      val trace = JSONArray()
      var resolvedHost: String? = null
      var resolvedPort: Int = 0
      var resolvedName: String? = null

      val discoveryListener = object : NsdManager.DiscoveryListener {
        override fun onStartDiscoveryFailed(serviceType: String?, errorCode: Int) {
          trace.put("onStartDiscoveryFailed: code=$errorCode")
          latch.countDown()
        }

        override fun onStopDiscoveryFailed(serviceType: String?, errorCode: Int) {
          trace.put("onStopDiscoveryFailed: code=$errorCode")
        }

        override fun onDiscoveryStarted(serviceType: String?) {
          val dt = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
          trace.put(String.format("%.2fms: onDiscoveryStarted(%s)", dt, serviceType))
        }

        override fun onDiscoveryStopped(serviceType: String?) {
          val dt = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
          trace.put(String.format("%.2fms: onDiscoveryStopped(%s)", dt, serviceType))
        }

        override fun onServiceFound(serviceInfo: NsdServiceInfo?) {
          val dt = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
          trace.put(String.format("%.2fms: onServiceFound(%s)", dt, serviceInfo?.serviceName))
          if (serviceInfo != null) {
            nsdManager.resolveService(serviceInfo, object : NsdManager.ResolveListener {
              override fun onResolveFailed(si: NsdServiceInfo?, errorCode: Int) {
                val dtR = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
                trace.put(String.format("%.2fms: onResolveFailed(code=%d)", dtR, errorCode))
              }

              override fun onServiceResolved(si: NsdServiceInfo?) {
                val dtR = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
                resolvedName = si?.serviceName
                resolvedHost = si?.host?.hostAddress
                resolvedPort = si?.port ?: 0
                trace.put(String.format("%.2fms: onServiceResolved(%s -> %s:%d)", dtR, resolvedName, resolvedHost, resolvedPort))
                latch.countDown()
              }
            })
          }
        }

        override fun onServiceLost(serviceInfo: NsdServiceInfo?) {
          trace.put("onServiceLost: ${serviceInfo?.serviceName}")
        }
      }

      try {
        nsdManager.discoverServices("_bookferry._tcp.", NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        latch.await(timeoutMs, TimeUnit.MILLISECONDS)
      } catch (e: Exception) {
        trace.put("Exception: ${e.message}")
      } finally {
        try {
          nsdManager.stopServiceDiscovery(discoveryListener)
        } catch (_: Exception) {}
      }

      val elapsedMs = (SystemClock.elapsedRealtimeNanos() - t0) / 1_000_000.0
      return JSONObject().apply {
        put("ok", resolvedHost != null)
        put("elapsed_ms", elapsedMs)
        put("service_name", resolvedName)
        put("host", resolvedHost)
        put("port", resolvedPort)
        put("multicast_lock_held", multicastLock?.isHeld == true)
        put("trace", trace)
      }.toString()
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
