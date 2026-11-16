// Interface imersiva do leitor: esconder toolbar/rodapé, tela cheia (desktop), teclado e
// teclas de volume (Android, via ponte nativa). A decisão de qual tecla faz o quê é pura e
// fica em state.js (`resolveKeyAction`).

import {
  arrowFallbackAction,
  arrowPanDelta,
  createWheelPager,
  normalizeWheelDelta,
  resolveKeyAction,
} from './state.js';

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
 *   openHelp?: () => void,
 *   openGoto?: () => void,
 * }} deps
 */
export function createReaderChrome({
  api,
  getReader,
  getAaPanel,
  closeReader,
  isDesktop,
  openHelp,
  openGoto,
}) {
  const screen = () => document.getElementById('reader-screen');
  let immersive = false;
  let fullscreen = false;
  let active = false;
  const pagePager = createWheelPager();
  const zoomPager = createWheelPager({ threshold: 40, cooldownMs: 90 });

  function syncFullscreenButton() {
    const btn = document.getElementById('btn-reader-fullscreen');
    if (!btn) return;
    btn.classList.toggle('hidden', !isDesktop());
    btn.setAttribute('aria-pressed', String(fullscreen));
  }

  function setImmersive(on) {
    const next = Boolean(on);
    const apply = () => {
      immersive = next;
      screen()?.classList.toggle('immersive', immersive);
      if (immersive) {
        getAaPanel()?.close?.();
        document.getElementById('toc-drawer')?.classList.add('hidden');
      }
      try {
        nativeBridge()?.setImmersive?.(immersive);
      } catch (_) {}
    };
    // O EPUB reflui ao mudar a altura da área de leitura: mantém o ponto de leitura.
    const reader = getReader();
    if (reader?.keepPositionDuring && next !== immersive) reader.keepPositionDuring(apply);
    else apply();
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

  /** Primeira/última página: PDF e quadrinhos por página, EPUB por fração do livro. */
  function seekEdge(reader, fraction) {
    if (reader?.seekPage) reader.seekPage(fraction === 0 ? 1 : Number.MAX_SAFE_INTEGER);
    else reader?.seekFraction?.(fraction);
  }

  /** Zoom: PDF/quadrinhos ampliam a página; no EPUB o "zoom" é o tamanho da fonte. */
  function zoom(reader, sign) {
    if (reader?.zoomIn && reader?.zoomOut) {
      if (sign > 0) reader.zoomIn();
      else reader.zoomOut();
    } else {
      getAaPanel()?.adjustFontSize?.(sign * 2);
    }
  }

  /**
   * Roda do mouse nos leitores. Ctrl/Cmd + roda dá zoom; a roda sozinha vira a página (uma por
   * "marcha", com cooldown para a inércia do trackpad). PDF/quadrinhos só chamam isto quando a
   * página não está ampliada (ampliada, a roda a rola); no EPUB vale sempre.
   * @param {WheelEvent} ev
   * @returns {boolean} true se tratou o evento
   */
  function handleWheel(ev) {
    if (!active || ev.defaultPrevented) return false;
    const reader = getReader();
    if (!reader) return false;
    const delta = normalizeWheelDelta(ev);
    const now = typeof performance !== 'undefined' ? performance.now() : Date.now();
    if (ev.ctrlKey || ev.metaKey) {
      ev.preventDefault();
      const dir = zoomPager.feed(-delta, now);
      if (dir) zoom(reader, dir);
      return true;
    }
    if (Math.abs(ev.deltaX || 0) > Math.abs(ev.deltaY || 0)) return false;
    ev.preventDefault();
    const dir = pagePager.feed(delta, now);
    if (dir > 0) reader.next?.();
    else if (dir < 0) reader.prev?.();
    return true;
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
    let action = resolveKeyAction(ev);
    if (!action) return false;
    ev.preventDefault();
    const direction = reader?.getDirection?.() || 'ltr';
    // Setas: com a página ampliada elas a movem; sem zoom viram virada de página.
    if (action.startsWith('arrow-')) {
      const pan = arrowPanDelta(action);
      if (pan && reader?.panBy?.(pan.dx, pan.dy)) return true;
      action = arrowFallbackAction(action, direction);
    }
    switch (action) {
      case 'next':
        reader?.next?.();
        break;
      case 'prev':
        reader?.prev?.();
        break;
      case 'first':
        seekEdge(reader, 0);
        break;
      case 'last':
        seekEdge(reader, 1);
        break;
      case 'zoom-in':
        zoom(reader, 1);
        break;
      case 'zoom-out':
        zoom(reader, -1);
        break;
      case 'zoom-reset':
        if (reader?.zoomReset) reader.zoomReset();
        else getAaPanel()?.resetFontSize?.();
        break;
      case 'toggle-fit':
        // Mesmo caminho do botão da barra: ele também atualiza o rótulo.
        if (reader?.toggleFitMode) document.getElementById('btn-fit-toggle')?.click();
        break;
      case 'toggle-toc':
        document.getElementById('btn-reader-toc')?.click();
        break;
      case 'goto':
        openGoto?.();
        break;
      case 'help':
        openHelp?.();
        break;
      case 'toggle-immersive':
        setImmersive(!immersive);
        break;
      case 'toggle-fullscreen':
        setFullscreen(!fullscreen);
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
    handleWheel,
    toggleImmersive: () => setImmersive(!immersive),
    setImmersive,
    toggleFullscreen: () => setFullscreen(!fullscreen),
    isImmersive: () => immersive,
    isFullscreen: () => fullscreen,
    /** Leitor aberto: liga as teclas de volume (Android). */
    open() {
      active = true;
      pagePager.reset();
      zoomPager.reset();
      syncFullscreenButton();
      window.__bookferryVolumeKey = (dir) => {
        const reader = getReader();
        // Volume + avança, volume − volta (preferência do Lucas, testada no S23).
        if (dir === 'up') reader?.next?.();
        else if (dir === 'down') reader?.prev?.();
      };
      try {
        nativeBridge()?.setVolumeKeysPaging?.(true);
        // Android: durante a leitura a tela não apaga.
        nativeBridge()?.setKeepScreenOn?.(true);
      } catch (_) {}
    },
    /** Leitor fechado: restaura interface, tela cheia e volume do sistema. */
    async close() {
      active = false;
      try {
        nativeBridge()?.setVolumeKeysPaging?.(false);
        nativeBridge()?.setKeepScreenOn?.(false);
      } catch (_) {}
      delete window.__bookferryVolumeKey;
      setImmersive(false);
      if (fullscreen) await setFullscreen(false);
    },
  };
}
