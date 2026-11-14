// Leitor PDF baseado em pdfjs-dist 4.10.38 (Apache-2.0) com HTTP Range via protocolo bookferry://
// e zoom/pan em duas fases (CSS transform + re-render hi-res no canvas) conforme medido no spike.

import * as pdfjsLib from '../../vendor/pdfjs/pdf.min.mjs';
import { getBookFileUrl } from '../api.js';
import {
  computePagePercent,
  createDebouncer,
  formatPageLocator,
  formatPercent,
  normalizeWheelDelta,
  panLimits,
  parsePageLocator,
  resolveTapZone,
} from '../state.js';

pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
  '../../vendor/pdfjs/pdf.worker.min.mjs',
  import.meta.url,
).href;

export async function mountPdfReader({
  book,
  initialPosition,
  viewportEl,
  statusLabelEl,
  onSavePosition,
  hooks = {},
}) {
  viewportEl.innerHTML = '<div class="reader-loading">Carregando PDF...</div>';

  let destroyed = false;
  let pdfDoc = null;
  let totalPages = 1;
  let currentPage = 1;
  let fitMode = 'page'; // 'page' | 'width'
  let zoomScale = 1.0;
  let renderedScale = 1.0;
  let tx = 0;
  let ty = 0;
  let activeRenderTask = null;
  let rerenderTimer = null;

  const debouncer = createDebouncer(async (pageNum) => {
    if (destroyed) return;
    const locator = formatPageLocator(pageNum);
    const percent = computePagePercent(pageNum, totalPages);
    await onSavePosition(book.book_id, locator, percent, `Página ${pageNum} de ${totalPages}`);
  }, 1500);

  const url = getBookFileUrl(book.book_id);
  const loadingTask = pdfjsLib.getDocument({
    url,
    rangeChunkSize: 65536,
    disableAutoFetch: true,
    disableStream: false,
  });
  pdfDoc = await loadingTask.promise;
  if (destroyed) {
    await pdfDoc.destroy();
    return null;
  }

  totalPages = Math.max(1, pdfDoc.numPages || 1);
  currentPage = initialPosition?.locator
    ? parsePageLocator(initialPosition.locator, totalPages)
    : 1;

  viewportEl.innerHTML = '';
  const stageEl = document.createElement('div');
  stageEl.className = 'zoom-stage';

  const canvasWrap = document.createElement('div');
  canvasWrap.className = 'zoom-target';

  const canvas = document.createElement('canvas');
  canvas.className = 'pdf-canvas';
  canvasWrap.appendChild(canvas);
  stageEl.appendChild(canvasWrap);
  viewportEl.appendChild(stageEl);

  function updateStatusLabel() {
    if (hooks.onProgress) {
      hooks.onProgress({
        kind: 'paged',
        page: currentPage,
        totalPages,
        zoomPct: Math.round(zoomScale * 100),
      });
      return;
    }
    if (!statusLabelEl) return;
    const pct = computePagePercent(currentPage, totalPages);
    const zoomPct = Math.round(zoomScale * 100);
    statusLabelEl.textContent = `Pág. ${currentPage} / ${totalPages} (${formatPercent(pct)}) · ${zoomPct}%`;
  }

  // Depois de renderizar uma página nova (ou mudar o encaixe), mostra o TOPO dela: no encaixe
  // "largura" a página é mais alta que a tela e, centralizada, abriria no meio.
  let pendingTopAlign = true;

  /** Limites do deslocamento (px) com o tamanho atual da página na tela. */
  function currentPanLimits() {
    const rel = zoomScale / renderedScale;
    return panLimits(
      canvas.offsetWidth * rel,
      canvas.offsetHeight * rel,
      stageEl.clientWidth,
      stageEl.clientHeight,
    );
  }

  /**
   * Move a página (roda do mouse / setas). Devolve true se ela se mexeu; false quando já está no
   * limite (ou cabe inteira na tela), e aí quem chamou vira a página.
   */
  function panBy(dx, dy) {
    const { limX, limY } = currentPanLimits();
    const nx = Math.max(-limX, Math.min(limX, tx + dx));
    const ny = Math.max(-limY, Math.min(limY, ty + dy));
    if (nx === tx && ny === ty) return false;
    tx = nx;
    ty = ny;
    applyCssTransform();
    return true;
  }

  function applyCssTransform() {
    const relativeScale = zoomScale / renderedScale;
    canvasWrap.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(${relativeScale})`;
  }

  async function renderCurrentPage(targetZoom = zoomScale) {
    if (destroyed || !pdfDoc) return;
    if (activeRenderTask) {
      try {
        activeRenderTask.cancel();
      } catch (_) {}
      activeRenderTask = null;
    }

    const page = await pdfDoc.getPage(currentPage);
    if (destroyed) return;

    const unscaledVp = page.getViewport({ scale: 1.0 });
    const availW = Math.max(280, (viewportEl.clientWidth || 800) - 24);
    const availH = Math.max(280, (viewportEl.clientHeight || 600) - 24);

    const baseScale =
      fitMode === 'width'
        ? availW / unscaledVp.width
        : Math.min(availW / unscaledVp.width, availH / unscaledVp.height);

    const dpr = Math.min(window.devicePixelRatio || 1, 2.5);
    const effectiveScale = baseScale * targetZoom;
    const hiResVp = page.getViewport({ scale: effectiveScale * dpr });

    canvas.width = Math.floor(hiResVp.width);
    canvas.height = Math.floor(hiResVp.height);
    canvas.style.width = `${Math.floor(unscaledVp.width * effectiveScale)}px`;
    canvas.style.height = `${Math.floor(unscaledVp.height * effectiveScale)}px`;

    const ctx = canvas.getContext('2d', { alpha: false });
    activeRenderTask = page.render({ canvasContext: ctx, viewport: hiResVp });
    try {
      await activeRenderTask.promise;
      renderedScale = targetZoom;
      if (pendingTopAlign) {
        pendingTopAlign = false;
        tx = 0;
        ty = currentPanLimits().limY;
      }
      canvasWrap.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(1)`;
    } catch (_) {
      // Renderização cancelada por nova mudança de página/zoom
    }
    updateStatusLabel();
  }

  function scheduleHighResRerender() {
    if (rerenderTimer) clearTimeout(rerenderTimer);
    rerenderTimer = setTimeout(() => {
      rerenderTimer = null;
      renderCurrentPage(zoomScale);
    }, 140);
  }

  function setZoom(newZoom) {
    const clamped = Math.max(0.75, Math.min(4.0, newZoom));
    zoomScale = Math.round(clamped * 100) / 100;
    if (zoomScale <= 1.02) {
      tx = 0;
      ty = 0;
      pendingTopAlign = true;
    }
    applyCssTransform();
    updateStatusLabel();
    scheduleHighResRerender();
  }

  // Gestos de Pointer Events (arrastar quando com zoom + pinch com 2 dedos) e Ctrl+Scroll
  const pointers = new Map();
  let lastPinchDist = null;
  let dragStart = null;
  let tapInfo = null; // toque simples: dedo único que quase não se mexeu

  stageEl.addEventListener('pointerdown', (ev) => {
    pointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    try {
      stageEl.setPointerCapture?.(ev.pointerId);
    } catch (_) {}
    if (pointers.size === 1) {
      dragStart = { x: ev.clientX - tx, y: ev.clientY - ty, rawX: ev.clientX };
      tapInfo = { x: ev.clientX, y: ev.clientY, moved: false };
    } else if (pointers.size === 2) {
      tapInfo = null;
      const pts = Array.from(pointers.values());
      lastPinchDist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
    }
  });

  stageEl.addEventListener('pointermove', (ev) => {
    if (!pointers.has(ev.pointerId)) return;
    pointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    if (tapInfo && Math.hypot(ev.clientX - tapInfo.x, ev.clientY - tapInfo.y) > 8) {
      tapInfo.moved = true;
    }

    if (pointers.size === 2) {
      const pts = Array.from(pointers.values());
      const dist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
      if (lastPinchDist && lastPinchDist > 10) {
        const factor = dist / lastPinchDist;
        setZoom(zoomScale * factor);
      }
      lastPinchDist = dist;
    } else if (pointers.size === 1 && dragStart && zoomScale > 1.02) {
      tx = ev.clientX - dragStart.x;
      ty = ev.clientY - dragStart.y;
      applyCssTransform();
    }
  });

  function onPointerEnd(ev) {
    pointers.delete(ev.pointerId);
    if (pointers.size < 2) lastPinchDist = null;
    if (pointers.size === 0) dragStart = null;
    if (ev.type === 'pointercancel') tapInfo = null;
  }

  stageEl.addEventListener('pointerup', (ev) => {
    onPointerEnd(ev);
    const tap = tapInfo;
    tapInfo = null;
    if (!tap || tap.moved || pointers.size !== 0) return;
    // Zonas de toque: 30% esquerda / 40% centro (alterna a interface) / 30% direita.
    // Com zoom o toque lateral não vira página (o dedo está arrastando a página ampliada).
    const rect = stageEl.getBoundingClientRect();
    const zone = zoomScale > 1.02 ? 'toggle' : resolveTapZone(ev.clientX - rect.left, rect.width, 'ltr');
    if (zone === 'toggle') hooks.onToggleChrome?.();
    else if (zone === 'prev' && currentPage > 1) goToPage(currentPage - 1, true);
    else if (zone === 'next' && currentPage < totalPages) goToPage(currentPage + 1, true);
  });
  stageEl.addEventListener('pointercancel', onPointerEnd);

  stageEl.addEventListener(
    'wheel',
    (ev) => {
      if (ev.ctrlKey || ev.metaKey) {
        ev.preventDefault();
        const delta = ev.deltaY < 0 ? 0.15 : -0.15;
        setZoom(zoomScale + delta);
        return;
      }
      // Roda: rola a página se ela passa da tela; no limite (ou se cabe inteira) vira a página.
      const horizontal = Math.abs(ev.deltaX) > Math.abs(ev.deltaY);
      const px = normalizeWheelDelta({ deltaY: horizontal ? ev.deltaX : ev.deltaY, deltaMode: ev.deltaMode });
      const moved = horizontal ? panBy(-px, 0) : panBy(0, -px);
      if (moved) ev.preventDefault();
      else if (!horizontal) hooks.onWheel?.(ev);
    },
    { passive: false },
  );

  await renderCurrentPage(zoomScale);

  async function goToPage(targetPage, triggerSave = true) {
    const nextP = Math.max(1, Math.min(totalPages, Math.floor(targetPage)));
    if (nextP === currentPage && triggerSave) return;
    currentPage = nextP;
    tx = 0;
    ty = 0;
    pendingTopAlign = true;
    await renderCurrentPage(zoomScale);
    if (triggerSave) {
      debouncer.trigger(currentPage);
    }
  }

  return {
    format: 'pdf',
    async next() {
      if (currentPage < totalPages) {
        await goToPage(currentPage + 1, true);
      }
    },
    async prev() {
      if (currentPage > 1) {
        await goToPage(currentPage - 1, true);
      }
    },
    /** Seek explícito (barra de progresso); `page` é 1-based. */
    async seekPage(page) {
      const target = Math.max(1, Math.min(totalPages, Math.round(Number(page)) || 1));
      if (target !== currentPage) await goToPage(target, true);
    },
    zoomIn() {
      setZoom(zoomScale + 0.25);
    },
    zoomOut() {
      setZoom(zoomScale - 0.25);
    },
    zoomReset() {
      setZoom(1.0);
    },
    panBy,
    toggleFitMode() {
      fitMode = fitMode === 'page' ? 'width' : 'page';
      zoomScale = 1.0;
      tx = 0;
      ty = 0;
      pendingTopAlign = true;
      renderCurrentPage(1.0);
      return fitMode;
    },
    async goToRemotePosition(remotePos) {
      if (!remotePos || !remotePos.locator) return;
      debouncer.cancel();
      const target = parsePageLocator(remotePos.locator, totalPages);
      currentPage = target;
      tx = 0;
      ty = 0;
      pendingTopAlign = true;
      await renderCurrentPage(zoomScale);
    },
    getCurrentState() {
      return {
        locator: formatPageLocator(currentPage),
        percent: computePagePercent(currentPage, totalPages),
        snippet: `Página ${currentPage} de ${totalPages}`,
      };
    },
    async flush() {
      await debouncer.flush();
    },
    async destroy() {
      destroyed = true;
      if (rerenderTimer) clearTimeout(rerenderTimer);
      await debouncer.flush();
      if (activeRenderTask) {
        try {
          activeRenderTask.cancel();
        } catch (_) {}
      }
      if (pdfDoc) {
        try {
          await pdfDoc.destroy();
        } catch (_) {}
      }
      viewportEl.innerHTML = '';
    },
  };
}
