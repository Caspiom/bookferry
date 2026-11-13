package dev.gaspari.bookferry

import android.os.Bundle
import android.view.KeyEvent
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import java.lang.ref.WeakReference

class MainActivity : TauriActivity() {
  // Quando true, as teclas de volume viram "página anterior/próxima" no leitor.
  // Escrita pela thread do JavascriptInterface, lida na main thread.
  @Volatile
  private var volumeKeysPaging = false

  // Referência fraca ao WebView (obtida em onWebViewCreate) para chamar o JS.
  private var webViewRef: WeakReference<WebView>? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webViewRef = WeakReference(webView)
    // Segurança: o WebView só carrega conteúdo do próprio app (assets/protocolo
    // bookferry com CSP restritiva), nunca páginas de terceiros; por isso é
    // aceitável expor esta ponte JavascriptInterface (API mínima: 2 métodos).
    webView.addJavascriptInterface(BookferryNativeBridge(), "BookferryNative")
  }

  override fun onDestroy() {
    webViewRef = null
    super.onDestroy()
  }

  // Intercepta volume +/- enquanto o leitor pede (flag ligada). Consome DOWN e UP
  // para o evento não vazar para o sistema (HUD de volume / mudança de volume).
  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    val code = event.keyCode
    if (volumeKeysPaging &&
      (code == KeyEvent.KEYCODE_VOLUME_UP || code == KeyEvent.KEYCODE_VOLUME_DOWN)
    ) {
      // Só o primeiro ACTION_DOWN vira avanço; repetições (segurar a tecla) são
      // consumidas mas ignoradas.
      if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
        val dir = if (code == KeyEvent.KEYCODE_VOLUME_UP) "up" else "down"
        webViewRef?.get()?.evaluateJavascript(
          "window.__bookferryVolumeKey && window.__bookferryVolumeKey('$dir')",
          null
        )
      }
      return true
    }
    return super.dispatchKeyEvent(event)
  }

  private fun applyImmersive(enabled: Boolean) {
    val controller = WindowCompat.getInsetsController(window, window.decorView)
    if (enabled) {
      controller.systemBarsBehavior =
        WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
      controller.hide(WindowInsetsCompat.Type.systemBars())
    } else {
      controller.show(WindowInsetsCompat.Type.systemBars())
    }
  }

  // Exposta ao JS como window.BookferryNative. Os métodos são chamados numa
  // thread de background do WebView: UI só via runOnUiThread.
  private inner class BookferryNativeBridge {
    @JavascriptInterface
    fun setVolumeKeysPaging(enabled: Boolean) {
      volumeKeysPaging = enabled
    }

    @JavascriptInterface
    fun setImmersive(enabled: Boolean) {
      runOnUiThread { applyImmersive(enabled) }
    }
  }
}
