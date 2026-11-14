// Leitor CBZ servindo entradas de imagem pelo protocolo bookferry://.
// Suporta: uma ou duas páginas lado a lado (spreads), direção LTR/RTL (mangá), encaixe
// (página/largura/altura), modo sem margens, zoom por pinça / toque duplo / Ctrl+roda,
// pan limitado às bordas e pré-carga do próximo spread. A matemática (pareamento, layout, pan,
// zonas de toque) é pura e fica em state.js.

import { api, getCbzPageUrl } from '../api.js';
import { createTapDispatcher } from '../gestures.js';
import {
  computePagePercent,
  computeSingleSpreads,
  computeSpreadLayout,
  computeSpreads,
  createDebouncer,
  formatPageLocator,
  initialPan,
  nextDoubleTapScale,
  normalizeWheelDelta,
  panLimits,
  parsePageLocator,
  resolveSpreadMode,
  resolveTapZone,
  sanitizeReaderPrefs,
  spreadIndexForPage,
  zoomTranslateForPoint,
} from '../state.js';

const STAGE_PADDING_PX = 12;
const FIT_ORDER = ['page', 'width', 'height'];
const PROBE_BATCH = 8;

export async function mountCbzReader({
  book,
  initialPosition,
  prefs: initialPrefs,
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
  let prefs = sanitizeReaderPrefs(initialPrefs);
  let currentPage = initialPosition?.locator
    ? parsePageLocator(initialPosition.locator, totalPages)
    : 1; // 1-based; em spread duplo é a primeira página do spread
  let zoomScale = 1.0;
  let tx = 0;
  let ty = 0;

  /** Tamanhos naturais já conhecidos (índice 0-based); null até a imagem carregar. */
  const dims = new Array(totalPages).fill(null);
  let spreads = computeSingleSpreads(totalPages);
  let spreadIdx = Math.max(0, currentPage - 1);
  let effectiveMode = 'single';
  let content = { contentW: 0, contentH: 0 };
  let renderToken = 0;
  let probeStarted = false;

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
  stageEl.appendChild(imgWrap);
  viewportEl.appendChild(stageEl);

  const preloaders = [];

  // ----- Utilidades de geometria -----
  function stageSize() {
    const r = stageEl.getBoundingClientRect();
    return { w: Math.max(1, r.width), h: Math.max(1, r.height) };
  }

  function availableArea() {
    const s = stageSize();
    const pad = prefs.zero_margin ? 0 : STAGE_PADDING_PX;
    return { w: Math.max(1, s.w - pad * 2), h: Math.max(1, s.h - pad * 2) };
  }

  const direction = () => prefs.direction;
  const spread = () => spreads[Math.min(spreadIdx, spreads.length - 1)];
  const pageNumberOfSpread = (sp) => (sp ? sp.pages[0] + 1 : 1);

  const overflowX = () => content.contentW * zoomScale > stageSize().w + 1;
  const overflowY = () => content.contentH * zoomScale > stageSize().h + 1;
  const overflows = () => overflowX() || overflowY();

  function clampPan() {
    const s = stageSize();
    const { limX, limY } = panLimits(
      content.contentW * zoomScale,
      content.contentH * zoomScale,
      s.w,
      s.h,
    );
    tx = Math.max(-limX, Math.min(limX, tx));
    ty = Math.max(-limY, Math.min(limY, ty));
  }

  function applyTransform() {
    imgWrap.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(${zoomScale})`;
  }

  function resetView() {
    zoomScale = 1.0;
    const s = stageSize();
    const p = initialPan(content.contentW, content.contentH, s.w, s.h, direction());
    tx = p.tx;
    ty = p.ty;
    applyTransform();
  }

  /**
   * Move a página (roda do mouse / setas). Devolve true se ela se mexeu; false quando já está no
   * limite (ou cabe inteira na tela), e aí quem chamou vira a página.
   */
  function panBy(dx, dy) {
    const before = { tx, ty };
    tx += dx;
    ty += dy;
    clampPan();
    if (tx === before.tx && ty === before.ty) return false;
    applyTransform();
    return true;
  }

  function updateStatusLabel() {
    const sp = spread();
    const first = sp.pages[0] + 1;
    const last = sp.pages[sp.pages.length - 1] + 1;
    if (hooks.onProgress) {
      hooks.onProgress({
        kind: 'paged',
        page: first,
        pageEnd: last > first ? last : undefined,
        totalPages,
        zoomPct: Math.round(zoomScale * 100),
      });
      return;
    }
    if (!statusLabelEl) return;
    statusLabelEl.textContent =
      last > first ? `Págs. ${first}–${last} / ${totalPages}` : `Pág. ${first} / ${totalPages}`;
  }

  // ----- Spreads -----
  function recomputeSpreads() {
    const s = stageSize();
    effectiveMode = resolveSpreadMode(prefs.spread_mode, s.w, s.h, dims);
    spreads =
      effectiveMode === 'double'
        ? computeSpreads(dims, { coverAlone: prefs.cover_alone, direction: direction() })
        : computeSingleSpreads(totalPages);
    spreadIdx = spreadIndexForPage(spreads, currentPage - 1);
    if (effectiveMode === 'double') startProbe();
  }

  function loadDims(i) {
    return new Promise((resolve) => {
      const im = new Image();
      im.onload = () => resolve({ w: im.naturalWidth, h: im.naturalHeight });
      im.onerror = () => resolve(null);
      im.src = getCbzPageUrl(book.book_id, i);
    });
  }

  /** Reagrupa os spreads sem sair da página atual; só redesenha se o spread mudou. */
  function rebuildKeepingPage() {
    const before = spread().pages.join(',');
    recomputeSpreads();
    if (spread().pages.join(',') !== before) renderSpread(false);
  }

  /**
   * Descobre as dimensões das páginas (só no modo duplo) para o pareamento tratar páginas largas.
   * Enquanto não conhece uma página, ela conta como retrato (documentado em computeSpreads).
   */
  async function startProbe() {
    if (probeStarted) return;
    probeStarted = true;
    let sinceRebuild = 0;
    for (let i = 0; i < totalPages && !destroyed; i += 1) {
      if (dims[i]) continue;
      // eslint-disable-next-line no-await-in-loop
      const d = await loadDims(i);
      if (destroyed) return;
      if (d) dims[i] = d;
      sinceRebuild += 1;
      if (sinceRebuild >= PROBE_BATCH) {
        sinceRebuild = 0;
        rebuildKeepingPage();
      }
    }
    if (!destroyed) rebuildKeepingPage();
  }

  // ----- Renderização -----
  async function renderSpread(triggerSave) {
    const token = ++renderToken;
    const sp = spread();
    currentPage = pageNumberOfSpread(sp);
    const imgs = sp.display.map((pageIdx) => {
      const im = document.createElement('img');
      im.className = 'cbz-image';
      im.alt = `Página ${pageIdx + 1} de ${totalPages}`;
      im.draggable = false;
      im.style.maxWidth = 'none';
      im.style.maxHeight = 'none';
      im.style.display = 'block';
      im.src = getCbzPageUrl(book.book_id, pageIdx);
      return { im, pageIdx };
    });
    await Promise.all(
      imgs.map(
        ({ im }) =>
          new Promise((resolve) => {
            if (im.complete && im.naturalWidth > 0) resolve();
            else {
              im.onload = () => resolve();
              im.onerror = () => resolve();
            }
          }),
      ),
    );
    if (destroyed || token !== renderToken) return;
    for (const { im, pageIdx } of imgs) {
      if (im.naturalWidth > 0) dims[pageIdx] = { w: im.naturalWidth, h: im.naturalHeight };
    }
    imgWrap.replaceChildren(...imgs.map((x) => x.im));
    layoutCurrent();
    resetView();
    updateStatusLabel();
    preloadNext();
    if (triggerSave) debouncer.trigger(currentPage);
  }

  /** Dimensiona as imagens do spread atual conforme encaixe e área disponível. */
  function layoutCurrent() {
    const sp = spread();
    const sizes = sp.display.map((i) => dims[i] || { w: 1, h: 1.4 });
    const layout = computeSpreadLayout(sizes, availableArea(), prefs.fit);
    content = { contentW: layout.contentW, contentH: layout.contentH };
    imgWrap.style.width = `${layout.contentW}px`;
    imgWrap.style.height = `${layout.contentH}px`;
    imgWrap.style.flex = 'none';
    [...imgWrap.children].forEach((im, k) => {
      const p = layout.pages[k];
      if (!p) return;
      im.style.width = `${p.w}px`;
      im.style.height = `${p.h}px`;
    });
    imgWrap.style.boxShadow = prefs.zero_margin ? 'none' : '';
    stageEl.style.background = { black: '#000', light: '#f4f1ea' }[prefs.bg] ?? '';
    clampPan();
    applyTransform();
  }

  function preloadNext() {
    preloaders.length = 0;
    const nextSp = spreads[spreadIdx + 1];
    if (!nextSp) return;
    for (const pageIdx of nextSp.pages) {
      const im = new Image();
      im.src = getCbzPageUrl(book.book_id, pageIdx);
      preloaders.push(im);
    }
  }

  function goToSpread(idx, triggerSave = true) {
    const clamped = Math.max(0, Math.min(spreads.length - 1, idx));
    if (clamped === spreadIdx && imgWrap.children.length > 0) return;
    spreadIdx = clamped;
    renderSpread(triggerSave);
  }

  function goToPage(pageNumber, triggerSave) {
    const p = Math.max(1, Math.min(totalPages, Math.floor(pageNumber)));
    currentPage = p;
    spreadIdx = spreadIndexForPage(spreads, p - 1);
    renderSpread(triggerSave);
  }

  // ----- Zoom -----
  function setZoom(newZoom, anchor) {
    const next = Math.round(Math.max(0.8, Math.min(4.0, newZoom)) * 100) / 100;
    if (anchor) {
      const s = stageSize();
      const t = zoomTranslateForPoint({
        px: anchor.x,
        py: anchor.y,
        stageW: s.w,
        stageH: s.h,
        contentW: content.contentW,
        contentH: content.contentH,
        fromScale: zoomScale,
        toScale: next,
        tx,
        ty,
      });
      tx = t.tx;
      ty = t.ty;
    }
    zoomScale = next;
    if (zoomScale <= 1.02 && !overflows()) {
      tx = 0;
      ty = 0;
    }
    clampPan();
    applyTransform();
    updateStatusLabel();
  }

  // ----- Gestos -----
  const pointers = new Map();
  let lastPinchDist = null;
  let down = null;

  /** +1 avança na ordem de leitura, -1 volta (a direção LTR/RTL já está nas zonas e no swipe). */
  const turn = (sign) => goToSpread(spreadIdx + sign);

  function handleZone(zone) {
    if (zone === 'toggle') hooks.onToggleChrome?.();
    else if (zone === 'next') turn(1);
    else if (zone === 'prev') turn(-1);
  }

  const taps = createTapDispatcher({
    onSingle: (pt) => {
      const rect = stageEl.getBoundingClientRect();
      // Com zoom, o toque lateral não vira página (o dedo está arrastando a página ampliada).
      const zone =
        zoomScale > 1.02 ? 'toggle' : resolveTapZone(pt.x - rect.left, rect.width, direction());
      handleZone(zone);
    },
    onDouble: (pt) => {
      const rect = stageEl.getBoundingClientRect();
      const target = nextDoubleTapScale(zoomScale);
      if (target === 1) {
        resetView();
        updateStatusLabel();
      } else {
        setZoom(target, { x: pt.x - rect.left, y: pt.y - rect.top });
      }
    },
  });

  stageEl.addEventListener('pointerdown', (ev) => {
    pointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    try {
      stageEl.setPointerCapture?.(ev.pointerId);
    } catch (_) {}
    if (pointers.size === 1) {
      down = { startX: ev.clientX, startY: ev.clientY, baseTx: tx, baseTy: ty, moved: false };
    } else if (pointers.size === 2) {
      down = null;
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
      if (lastPinchDist && lastPinchDist > 10) setZoom(zoomScale * (dist / lastPinchDist));
      lastPinchDist = dist;
    } else if (pointers.size === 1 && down) {
      const dx = ev.clientX - down.startX;
      const dy = ev.clientY - down.startY;
      if (Math.hypot(dx, dy) > 10) down.moved = true;
      if (overflows() && down.moved) {
        if (overflowX()) tx = down.baseTx + dx;
        if (overflowY()) ty = down.baseTy + dy;
        clampPan();
        applyTransform();
      }
    }
  });

  stageEl.addEventListener('pointerup', (ev) => {
    pointers.delete(ev.pointerId);
    if (pointers.size < 2) lastPinchDist = null;
    if (pointers.size !== 0 || !down) return;
    const dx = ev.clientX - down.startX;
    const dy = ev.clientY - down.startY;
    const wasMoved = down.moved;
    down = null;
    // Swipe horizontal vira página quando não há o que arrastar na horizontal.
    if (!overflowX() && Math.abs(dx) > 55 && Math.abs(dx) > Math.abs(dy) * 1.4) {
      taps.cancel();
      // LTR: arrastar para a esquerda avança; RTL: arrastar para a direita avança.
      const forward = direction() === 'rtl' ? dx > 0 : dx < 0;
      turn(forward ? 1 : -1);
      return;
    }
    if (!wasMoved) taps.tap(ev.clientX, ev.clientY);
  });

  stageEl.addEventListener('pointercancel', (ev) => {
    pointers.delete(ev.pointerId);
    lastPinchDist = null;
    down = null;
  });

  stageEl.addEventListener(
    'wheel',
    (ev) => {
      if (ev.ctrlKey || ev.metaKey) {
        ev.preventDefault();
        const rect = stageEl.getBoundingClientRect();
        // Proporcional ao delta, com teto de um passo do teclado (ver pdf.js).
        const dz = -normalizeWheelDelta(ev) * 0.005;
        setZoom(zoomScale + Math.max(-0.25, Math.min(0.25, dz)), {
          x: ev.clientX - rect.left,
          y: ev.clientY - rect.top,
        });
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

  const resizeObserver = new ResizeObserver(() => {
    if (destroyed || imgWrap.children.length === 0) return;
    const before = spread().pages.join(',');
    recomputeSpreads();
    if (spread().pages.join(',') !== before) {
      renderSpread(false);
    } else {
      layoutCurrent();
      if (zoomScale <= 1.02) resetView();
    }
  });
  resizeObserver.observe(stageEl);

  // ----- Preferências de quadrinhos -----
  function changePrefs(patch) {
    prefs = sanitizeReaderPrefs({ ...prefs, ...patch });
    recomputeSpreads();
    renderSpread(false);
    hooks.onPrefsChange?.(prefs);
  }

  recomputeSpreads();
  await renderSpread(false);

  return {
    format: 'cbz',
    getDirection: () => direction(),
    getPrefs: () => ({ ...prefs }),
    getEffectiveSpreadMode: () => effectiveMode,
    async next() {
      turn(1);
    },
    async prev() {
      turn(-1);
    },
    /** Seek explícito (barra de progresso); `page` é 1-based. */
    async seekPage(page) {
      const target = Math.max(1, Math.min(totalPages, Math.round(Number(page)) || 1));
      if (target !== currentPage) goToPage(target, true);
    },
    zoomIn() {
      setZoom(zoomScale + 0.25);
    },
    zoomOut() {
      setZoom(zoomScale - 0.25);
    },
    zoomReset() {
      resetView();
      updateStatusLabel();
    },
    panBy,
    /** Alterna o encaixe (página -> largura -> altura) e devolve o modo novo. */
    toggleFitMode() {
      changePrefs({ fit: FIT_ORDER[(FIT_ORDER.indexOf(prefs.fit) + 1) % FIT_ORDER.length] });
      return prefs.fit;
    },
    toggleDirection() {
      changePrefs({ direction: prefs.direction === 'rtl' ? 'ltr' : 'rtl' });
      return prefs.direction;
    },
    cycleSpreadMode() {
      const order = ['single', 'double', 'auto'];
      changePrefs({ spread_mode: order[(order.indexOf(prefs.spread_mode) + 1) % order.length] });
      return prefs.spread_mode;
    },
    toggleZeroMargin() {
      changePrefs({ zero_margin: !prefs.zero_margin });
      return prefs.zero_margin;
    },
    toggleCoverAlone() {
      changePrefs({ cover_alone: !prefs.cover_alone });
      return prefs.cover_alone;
    },
    async goToRemotePosition(remotePos) {
      if (!remotePos || !remotePos.locator) return;
      debouncer.cancel();
      goToPage(parsePageLocator(remotePos.locator, totalPages), false);
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
      resizeObserver.disconnect();
      taps.cancel();
      await debouncer.flush();
      viewportEl.innerHTML = '';
    },
  };
}
