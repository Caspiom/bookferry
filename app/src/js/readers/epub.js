// Leitor EPUB baseado em foliate-js (MIT).
// Regra crítica medida no spike (docs/spike/02-riscos.md):
// Restaurar SEMPRE com `view.goTo(cfi)` (que usa resolveCFI internamente) e NUNCA `goToFraction`.
// Colapsar CFIs de range com `CFI.collapse(cfi)` antes de persistir.

import '../../vendor/foliate-js/view.js';
import * as CFI from '../../vendor/foliate-js/epubcfi.js';
import { getBookFileUrl } from '../api.js';
import { createTapDispatcher } from '../gestures.js';
import {
  buildEpubCss,
  createDebouncer,
  epubLayoutAttrs,
  isEpubCfi,
  resolveEffectiveTheme,
  resolveTapZone,
  sanitizeReaderPrefs,
} from '../state.js';

export async function mountEpubReader({
  book,
  initialPosition,
  prefs: initialPrefs,
  appTheme = 'auto',
  viewportEl,
  tocListEl,
  onSavePosition,
  hooks = {},
}) {
  viewportEl.innerHTML = '<div class="reader-loading">Carregando EPUB...</div>';

  let prefs = sanitizeReaderPrefs(initialPrefs);
  const prefersDark = () =>
    typeof window !== 'undefined' &&
    Boolean(window.matchMedia) &&
    window.matchMedia('(prefers-color-scheme: dark)').matches;
  const effectiveTheme = () => resolveEffectiveTheme(prefs.theme, appTheme, prefersDark());
  // Enquanto o estilo muda, os `relocate` programáticos (reflow) não podem mover nem salvar a
  // posição: o ponto de leitura continua sendo o último CFI "do usuário".
  let restyling = false;
  let restyleTimer = null;
  let applying = false;
  let pendingPrefs = null;

  let currentLocator = initialPosition?.locator || '';
  let currentPercent = Number(initialPosition?.percent) || 0;
  let currentSnippet = initialPosition?.snippet || null;
  let suppressSave = true;
  let destroyed = false;

  const debouncer = createDebouncer(async (locator, percent, snippet) => {
    if (destroyed || !locator) return;
    await onSavePosition(book.book_id, locator, percent, snippet);
  }, 1500);

  // Busca o EPUB via protocolo customizado bookferry://
  const url = getBookFileUrl(book.book_id);
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(`Falha ao abrir EPUB via protocolo local (HTTP ${resp.status})`);
  }
  const blob = await resp.blob();
  const file = new File([blob], `${book.book_id}.epub`, { type: 'application/epub+zip' });

  if (destroyed) return null;
  viewportEl.innerHTML = '';

  const view = document.createElement('foliate-view');
  view.style.width = '100%';
  view.style.height = '100%';
  view.style.display = 'block';
  viewportEl.appendChild(view);

  await view.open(file);
  const renderer = view.renderer;

  function applyLayoutAndStyles() {
    for (const [attr, value] of Object.entries(epubLayoutAttrs(prefs))) {
      renderer?.setAttribute?.(attr, value);
    }
    renderer?.setStyles?.(buildEpubCss(prefs, effectiveTheme()));
    // `gap` só atualiza uma variável CSS: força o re-layout (mantém a âncora atual).
    renderer?.render?.();
    hooks.onThemeChange?.(effectiveTheme());
  }
  applyLayoutAndStyles();

  const bookDirection = () => (view.book?.dir === 'rtl' ? 'rtl' : 'ltr');

  // Toque/clique no documento do livro (iframe do foliate) -> mesma lógica de zonas do app.
  const tapper = createTapDispatcher({
    delayMs: 220,
    onSingle: ({ x }) => {
      const width = view.getBoundingClientRect().width;
      const zone = resolveTapZone(x, width, bookDirection());
      if (zone === 'toggle') {
        hooks.onToggleChrome?.();
        return;
      }
      suppressSave = false;
      (zone === 'next' ? view.next() : view.prev())?.catch?.(() => {});
    },
  });
  view.addEventListener('load', (ev) => {
    const doc = ev.detail?.doc;
    if (!doc) return;
    doc.addEventListener('keydown', (kev) => hooks.onKey?.(kev));
    doc.addEventListener('click', (cev) => {
      hooks.onDismissOverlays?.();
      // Link do livro (o foliate trata e dá preventDefault) ou controle: não vira navegação.
      if (cev.defaultPrevented || cev.target?.closest?.('a[href], button, input, select, textarea')) {
        return;
      }
      // Seleção de texto em andamento/feita: não vira navegação nem alterna o imersivo.
      const sel = doc.getSelection?.();
      if (sel && sel.type === 'Range' && sel.toString().length > 0) return;
      // Coordenadas do iframe (largo, rolado por página) -> coordenadas da view visível.
      const frame = doc.defaultView?.frameElement;
      const frameLeft = frame ? frame.getBoundingClientRect().left : 0;
      const viewLeft = view.getBoundingClientRect().left;
      tapper.tap(frameLeft + cev.clientX - viewLeft, cev.clientY);
    });
  });

  // Preenche o sumário (TOC)
  if (tocListEl) {
    tocListEl.innerHTML = '';
    const tocItems = view.book?.toc || [];
    if (tocItems.length === 0) {
      tocListEl.innerHTML = '<li class="muted-note">Sumário indisponível neste EPUB.</li>';
    } else {
      const renderItems = (items, depth = 0) => {
        for (const item of items) {
          const li = document.createElement('li');
          const btn = document.createElement('button');
          btn.type = 'button';
          btn.className = 'toc-item-btn';
          btn.style.paddingLeft = `${12 + depth * 14}px`;
          btn.textContent = item.label || item.href || 'Seção';
          btn.addEventListener('click', async () => {
            if (item.href) {
              suppressSave = false;
              await view.goTo(item.href);
              document.getElementById('toc-drawer')?.classList.add('hidden');
            }
          });
          li.appendChild(btn);
          tocListEl.appendChild(li);
          if (Array.isArray(item.subitems) && item.subitems.length > 0) {
            renderItems(item.subitems, depth + 1);
          }
        }
      };
      renderItems(tocItems, 0);
    }
  }

  function emitProgress(d) {
    if (!hooks.onProgress) return;
    // Capítulo = seção (arquivo) do EPUB: só é calculado de forma confiável o que o paginator
    // sabe (página atual/total da seção carregada). Sem isso (layout fixo) os itens são omitidos.
    let chapterFraction = null;
    let pagesLeftInChapter = null;
    const pages = Number(renderer?.pages);
    const page = Number(renderer?.page);
    if (Number.isFinite(pages) && Number.isFinite(page) && pages >= 3) {
      const textPages = pages - 2;
      const cur = Math.max(1, Math.min(textPages, page));
      chapterFraction = cur / textPages;
      pagesLeftInChapter = textPages - cur;
    }
    hooks.onProgress({
      kind: 'epub',
      chapterTitle: d.tocItem?.label || '',
      bookFraction: typeof d.fraction === 'number' ? d.fraction : null,
      chapterFraction,
      pagesLeftInChapter,
      totalChapters: d.section?.total ?? null,
    });
  }

  view.addEventListener('relocate', (ev) => {
    if (destroyed) return;
    const d = ev.detail || {};
    emitProgress(d);
    if (restyling) return;
    if (d.cfi) {
      try {
        currentLocator = d.cfi.includes(',') ? CFI.collapse(d.cfi) : d.cfi;
      } catch (_) {
        currentLocator = d.cfi;
      }
    }
    if (typeof d.fraction === 'number' && Number.isFinite(d.fraction)) {
      // Garante pelo menos 1% quando já abriu o livro e avançou
      const rawPct = Math.max(0.5, d.fraction * 100);
      currentPercent = Math.round(rawPct * 100) / 100;
    }
    currentSnippet = d.tocItem?.label || currentSnippet || null;

    if (!suppressSave && currentLocator) {
      debouncer.trigger(currentLocator, currentPercent, currentSnippet);
    }
  });

  // Restaura posição inicial por CFI (NUNCA goToFraction)
  if (initialPosition && isEpubCfi(initialPosition.locator)) {
    try {
      await view.goTo(initialPosition.locator);
    } catch (_) {
      await view.renderer?.next?.();
    }
  } else {
    await view.renderer?.next?.();
  }

  // Libera persistência para interações subsequentes do usuário
  setTimeout(() => {
    suppressSave = false;
  }, 150);

  return {
    format: 'epub',
    async next() {
      suppressSave = false;
      await view.next();
    },
    async prev() {
      suppressSave = false;
      await view.prev();
    },
    getDirection: bookDirection,
    /** Tema do app (Ajustes) mudou: só afeta o EPUB quando o tema do livro é "auto". */
    setAppTheme(next) {
      appTheme = next || 'auto';
      if (prefs.theme === 'auto') applyLayoutAndStyles();
    },
    getPrefs: () => ({ ...prefs }),
    /**
     * Aplica novas preferências (pré-visualização ao vivo) mantendo o mesmo ponto de leitura:
     * o último CFI do usuário é restaurado com `view.goTo(cfi)` após o reflow e nada é salvo.
     */
    async applyPrefs(next) {
      if (applying) {
        pendingPrefs = next;
        return;
      }
      applying = true;
      try {
        let todo = next;
        while (todo) {
          pendingPrefs = null;
          prefs = sanitizeReaderPrefs(todo);
          restyling = true;
          if (restyleTimer) clearTimeout(restyleTimer);
          const anchor = currentLocator;
          applyLayoutAndStyles();
          if (isEpubCfi(anchor)) {
            try {
              await view.goTo(anchor);
            } catch (_) {}
          }
          restyleTimer = setTimeout(() => {
            restyling = false;
          }, 450);
          todo = pendingPrefs;
        }
      } finally {
        applying = false;
      }
    },
    /**
     * Executa uma ação que redimensiona a área de leitura (ex.: esconder a toolbar no modo
     * imersivo) e devolve o leitor ao mesmo ponto: o reflow do paginator pode cair numa página
     * vizinha, e a posição salva não pode andar por causa disso.
     */
    async keepPositionDuring(action) {
      const anchor = currentLocator;
      restyling = true;
      if (restyleTimer) clearTimeout(restyleTimer);
      action();
      await new Promise((resolve) => setTimeout(resolve, 350)); // deixa o ResizeObserver refluir
      if (isEpubCfi(anchor)) {
        try {
          await view.goTo(anchor);
        } catch (_) {}
      }
      restyling = false;
    },
    /** Seek explícito pela barra de progresso (a posição salva continua sendo o CFI do relocate). */
    async seekFraction(fraction) {
      const f = Math.max(0, Math.min(1, Number(fraction) || 0));
      suppressSave = false;
      restyling = false;
      await view.goToFraction(f);
    },
    /** Apoio ao E2E: documento do livro e se o ponto do CFI está na página visível. */
    getContentDoc() {
      return renderer?.getContents?.()[0]?.doc || null;
    },
    anchorIsVisible(cfi) {
      try {
        const resolved = view.resolveCFI(cfi);
        const doc = renderer.getContents()[0].doc;
        const point = resolved.anchor(doc);
        const vis = view.lastLocation?.range;
        if (!vis || !point) return false;
        const p = point.startContainer ? point : (() => {
          const r = doc.createRange();
          r.selectNode(point);
          return r;
        })();
        return (
          vis.compareBoundaryPoints(Range.START_TO_START, p) <= 0 &&
          vis.compareBoundaryPoints(Range.END_TO_END, p) >= 0
        );
      } catch (_) {
        return false;
      }
    },
    async goToRemotePosition(remotePos) {
      if (!remotePos || !isEpubCfi(remotePos.locator)) return;
      suppressSave = true;
      restyling = false;
      debouncer.cancel();
      try {
        await view.goTo(remotePos.locator);
        currentLocator = remotePos.locator;
        currentPercent = Number(remotePos.percent) || currentPercent;
      } finally {
        setTimeout(() => {
          suppressSave = false;
        }, 150);
      }
    },
    getCurrentState() {
      return {
        locator: currentLocator,
        percent: currentPercent,
        snippet: currentSnippet,
      };
    },
    async flush() {
      await debouncer.flush();
    },
    async destroy() {
      destroyed = true;
      tapper.cancel();
      if (restyleTimer) clearTimeout(restyleTimer);
      await debouncer.flush();
      try {
        view.close?.();
      } catch (_) {}
      viewportEl.innerHTML = '';
    },
  };
}
