// Interface imersiva do leitor: esconder toolbar/rodapé, tela cheia (desktop), teclado e
// teclas de volume (Android, via ponte nativa). A decisão de qual tecla faz o quê é pura e
// fica em state.js (`resolveKeyAction`).

import { resolveKeyAction } from './state.js';

/** Ponte nativa do Android (MainActivity.kt); ausente no desktop. */
const nativeBridge = () =>
  typeof window !== 'undefined' && window.BookferryNative ? window.BookferryNative : null;

/**
 * @param {{
 *   api: { setFullscreen: (b: boolean) => Promise<void> },
 *   getReader: () => object | null,
 *   getAaPanel: () => ({ isOpen: () => boolean, close: () => void, adjustFontSize?: (d: number) => void } | null),
 *   closeReader: () => void,
 *   isDesktop: () => boolean,
 * }} deps
 */
export function createReaderChrome({ api, getReader, getAaPanel, closeReader, isDesktop }) {
  const screen = () => document.getElementById('reader-screen');
  let immersive = false;
  let fullscreen = false;
  let active = false;

  function syncFullscreenButton() {
    const btn = document.getElementById('btn-reader-fullscreen');
    if (!btn) return;
    btn.classList.toggle('hidden', !isDesktop());
    btn.setAttribute('aria-pressed', String(fullscreen));
  }

  function setImmersive(on) {
    immersive = Boolean(on);
    screen()?.classList.toggle('immersive', immersive);
    if (immersive) {
      getAaPanel()?.close?.();
      document.getElementById('toc-drawer')?.classList.add('hidden');
    }
    try {
      nativeBridge()?.setImmersive?.(immersive);
    } catch (_) {}
  }

  async function setFullscreen(on) {
    if (!isDesktop()) return;
    try {
      await api.setFullscreen(Boolean(on));
      fullscreen = Boolean(on);
    } catch (_) {
      // Sem suporte (ex.: compositor sem tela cheia): o estado fica como estava.
    }
    syncFullscreenButton();
  }

  /** Esc: fecha o painel; senão sai da tela cheia; senão sai do imersivo; senão fecha o leitor. */
  async function escape() {
    const panel = getAaPanel();
    if (panel?.isOpen?.()) {
      panel.close();
      return;
    }
    if (fullscreen) {
      await setFullscreen(false);
      return;
    }
    if (immersive) {
      setImmersive(false);
      return;
    }
    closeReader();
  }

  /**
   * Trata uma tecla (da janela ou do documento do livro). Devolve true se consumiu.
   * @param {KeyboardEvent} ev
   */
  function handleKey(ev) {
    if (!active || ev.defaultPrevented) return false;
    const tag = ev.target?.tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') {
      // Esc dentro do painel é tratado pelo próprio painel; nada mais.
      return false;
    }
    // Espaço em botão focado é clique, não virar página.
    if (tag === 'BUTTON' && (ev.key === ' ' || ev.key === 'Spacebar' || ev.key === 'Enter')) {
      return false;
    }
    const reader = getReader();
    const action = resolveKeyAction(ev, reader?.getDirection?.() || 'ltr');
    if (!action) return false;
    ev.preventDefault();
    switch (action) {
      case 'next':
        reader?.next?.();
        break;
      case 'prev':
        reader?.prev?.();
        break;
      case 'toggle-immersive':
        setImmersive(!immersive);
        break;
      case 'toggle-fullscreen':
        setFullscreen(!fullscreen);
        break;
      case 'font-inc':
        getAaPanel()?.adjustFontSize?.(1);
        break;
      case 'font-dec':
        getAaPanel()?.adjustFontSize?.(-1);
        break;
      case 'escape':
        escape();
        break;
      default:
        return false;
    }
    return true;
  }

  return {
    handleKey,
    toggleImmersive: () => setImmersive(!immersive),
    setImmersive,
    toggleFullscreen: () => setFullscreen(!fullscreen),
    isImmersive: () => immersive,
    isFullscreen: () => fullscreen,
    /** Leitor aberto: liga as teclas de volume (Android). */
    open() {
      active = true;
      syncFullscreenButton();
      window.__bookferryVolumeKey = (dir) => {
        const reader = getReader();
        if (dir === 'up') reader?.prev?.();
        else if (dir === 'down') reader?.next?.();
      };
      try {
        nativeBridge()?.setVolumeKeysPaging?.(true);
      } catch (_) {}
    },
    /** Leitor fechado: restaura interface, tela cheia e volume do sistema. */
    async close() {
      active = false;
      try {
        nativeBridge()?.setVolumeKeysPaging?.(false);
      } catch (_) {}
      delete window.__bookferryVolumeKey;
      setImmersive(false);
      if (fullscreen) await setFullscreen(false);
    },
  };
}
