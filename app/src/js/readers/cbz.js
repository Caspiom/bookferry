// Leitor CBZ de página única servindo entradas de imagem pelo protocolo bookferry://
// com pré-carregamento da página n+1 e zoom/pan GPU por Pointer Events + CSS transform.

import { api, getCbzPageUrl } from '../api.js';
import {
  computePagePercent,
  createDebouncer,
  formatPageLocator,
  formatPercent,
  parsePageLocator,
  resolveTapZone,
} from '../state.js';

export async function mountCbzReader({
  book,
  initialPosition,
  viewportEl,
  statusLabelEl,
  onSavePosition,
  hooks = {},
}) {
  viewportEl.innerHTML = '<div class="reader-loading">Indexando páginas do CBZ...</div>';

  let destroyed = false;
  const info = await api.cbzInfo(book.book_id);
  if (destroyed) return null;

  const totalPages = Math.max(1, Number(info?.page_count) || 1);
  let currentPage = initialPosition?.locator
    ? parsePageLocator(initialPosition.locator, totalPages)
    : 1;
  let fitMode = 'page'; // 'page' (altura/tela) | 'width' (largura)
  let zoomScale = 1.0;
  let tx = 0;
  let ty = 0;

  const debouncer = createDebouncer(async (pageNum) => {
    if (destroyed) return;
    const locator = formatPageLocator(pageNum);
    const percent = computePagePercent(pageNum, totalPages);
    await onSavePosition(book.book_id, locator, percent, `Página ${pageNum} de ${totalPages}`);
  }, 1500);

  viewportEl.innerHTML = '';
  const stageEl = document.createElement('div');
  stageEl.className = 'zoom-stage cbz-stage';

  const imgWrap = document.createElement('div');
  imgWrap.className = 'zoom-target cbz-target';

  const imgEl = document.createElement('img');
  imgEl.className = 'cbz-image fit-page';
  imgEl.alt = `Página ${currentPage}`;
  imgEl.draggable = false;

  imgWrap.appendChild(imgEl);
  stageEl.appendChild(imgWrap);
  viewportEl.appendChild(stageEl);

  const preloadImg = new Image();

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

  function applyTransform() {
    imgWrap.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(${zoomScale})`;
  }

  function setZoom(newZoom) {
    const clamped = Math.max(0.8, Math.min(4.0, newZoom));
    zoomScale = Math.round(clamped * 100) / 100;
    if (zoomScale <= 1.02) {
      tx = 0;
      ty = 0;
    }
    applyTransform();
    updateStatusLabel();
  }

  function loadPage(pageNumber, triggerSave = false) {
    const clamped = Math.max(1, Math.min(totalPages, Math.floor(pageNumber)));
    currentPage = clamped;
    zoomScale = 1.0;
    tx = 0;
    ty = 0;
    applyTransform();

    const zeroIdx = currentPage - 1;
    imgEl.src = getCbzPageUrl(book.book_id, zeroIdx);
    imgEl.alt = `Página ${currentPage} de ${totalPages}`;
    updateStatusLabel();

    // Pré-carrega a próxima página (n + 1) em segundo plano
    if (currentPage < totalPages) {
      preloadImg.src = getCbzPageUrl(book.book_id, zeroIdx + 1);
    }

    if (triggerSave) {
      debouncer.trigger(currentPage);
    }
  }

  // Pointer Events: pinch zoom, pan quando com zoom, swipe horizontal e toque nas bordas (25%)
  const pointers = new Map();
  let lastPinchDist = null;
  let pointerDownInfo = null;

  stageEl.addEventListener('pointerdown', (ev) => {
    pointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    try {
      stageEl.setPointerCapture?.(ev.pointerId);
    } catch (_) {}
    if (pointers.size === 1) {
      pointerDownInfo = {
        startX: ev.clientX,
        startY: ev.clientY,
        baseTx: tx,
        baseTy: ty,
        moved: false,
      };
    } else if (pointers.size === 2) {
      pointerDownInfo = null;
      const pts = Array.from(pointers.values());
      lastPinchDist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
    }
  });

  stageEl.addEventListener('pointermove', (ev) => {
    if (!pointers.has(ev.pointerId)) return;
    pointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });

    if (pointers.size === 2) {
      const pts = Array.from(pointers.values());
      const dist = Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y);
      if (lastPinchDist && lastPinchDist > 10) {
        setZoom(zoomScale * (dist / lastPinchDist));
      }
      lastPinchDist = dist;
    } else if (pointers.size === 1 && pointerDownInfo) {
      const dx = ev.clientX - pointerDownInfo.startX;
      const dy = ev.clientY - pointerDownInfo.startY;
      if (Math.hypot(dx, dy) > 10) {
        pointerDownInfo.moved = true;
      }
      if (zoomScale > 1.02) {
        tx = pointerDownInfo.baseTx + dx;
        ty = pointerDownInfo.baseTy + dy;
        applyTransform();
      }
    }
  });

  stageEl.addEventListener('pointerup', (ev) => {
    pointers.delete(ev.pointerId);
    if (pointers.size < 2) lastPinchDist = null;

    if (pointers.size === 0 && pointerDownInfo) {
      const dx = ev.clientX - pointerDownInfo.startX;
      const dy = ev.clientY - pointerDownInfo.startY;

      if (zoomScale <= 1.02) {
        // Swipe horizontal
        if (Math.abs(dx) > 55 && Math.abs(dx) > Math.abs(dy) * 1.4) {
          if (dx < 0 && currentPage < totalPages) {
            loadPage(currentPage + 1, true);
          } else if (dx > 0 && currentPage > 1) {
            loadPage(currentPage - 1, true);
          }
        } else if (!pointerDownInfo.moved) {
          // Zonas de toque: 30% esquerda / 40% centro (alterna a interface) / 30% direita
          const rect = stageEl.getBoundingClientRect();
          const zone = resolveTapZone(ev.clientX - rect.left, rect.width, 'ltr');
          if (zone === 'toggle') {
            hooks.onToggleChrome?.();
          } else if (zone === 'prev' && currentPage > 1) {
            loadPage(currentPage - 1, true);
          } else if (zone === 'next' && currentPage < totalPages) {
            loadPage(currentPage + 1, true);
          }
        }
      } else if (!pointerDownInfo.moved) {
        hooks.onToggleChrome?.();
      }
      pointerDownInfo = null;
    }
  });

  stageEl.addEventListener('pointercancel', (ev) => {
    pointers.delete(ev.pointerId);
    lastPinchDist = null;
    pointerDownInfo = null;
  });

  stageEl.addEventListener(
    'wheel',
    (ev) => {
      if (ev.ctrlKey || ev.metaKey) {
        ev.preventDefault();
        const delta = ev.deltaY < 0 ? 0.15 : -0.15;
        setZoom(zoomScale + delta);
      }
    },
    { passive: false },
  );

  loadPage(currentPage, false);

  return {
    format: 'cbz',
    async next() {
      if (currentPage < totalPages) {
        loadPage(currentPage + 1, true);
      }
    },
    async prev() {
      if (currentPage > 1) {
        loadPage(currentPage - 1, true);
      }
    },
    /** Seek explícito (barra de progresso); `page` é 1-based. */
    async seekPage(page) {
      const target = Math.max(1, Math.min(totalPages, Math.round(Number(page)) || 1));
      if (target !== currentPage) loadPage(target, true);
    },
    zoomIn() {
      setZoom(zoomScale + 0.25);
    },
    zoomOut() {
      setZoom(zoomScale - 0.25);
    },
    toggleFitMode() {
      fitMode = fitMode === 'page' ? 'width' : 'page';
      imgEl.className = fitMode === 'width' ? 'cbz-image fit-width' : 'cbz-image fit-page';
      zoomScale = 1.0;
      tx = 0;
      ty = 0;
      applyTransform();
      return fitMode;
    },
    async goToRemotePosition(remotePos) {
      if (!remotePos || !remotePos.locator) return;
      debouncer.cancel();
      const target = parsePageLocator(remotePos.locator, totalPages);
      loadPage(target, false);
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
      await debouncer.flush();
      viewportEl.innerHTML = '';
    },
  };
}
