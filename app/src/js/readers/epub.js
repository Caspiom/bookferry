// Leitor EPUB baseado em foliate-js (MIT).
// Regra crítica medida no spike (docs/spike/02-riscos.md):
// Restaurar SEMPRE com `view.goTo(cfi)` (que usa resolveCFI internamente) e NUNCA `goToFraction`.
// Colapsar CFIs de range com `CFI.collapse(cfi)` antes de persistir.

import '../../vendor/foliate-js/view.js';
import * as CFI from '../../vendor/foliate-js/epubcfi.js';
import { api, getBookFileUrl } from '../api.js';
import { createDebouncer, formatPercent, isEpubCfi } from '../state.js';

function buildEpubStyles(fontSizePx, theme) {
  const prefersDark =
    typeof window !== 'undefined' &&
    window.matchMedia &&
    window.matchMedia('(prefers-color-scheme: dark)').matches;
  const effectiveTheme =
    !theme || theme === 'auto' ? (prefersDark ? 'dark' : 'light') : theme;

  let colorsCss = `
    :root { color-scheme: light; }
    html, body {
      background: #ffffff !important;
      color: #1c2026 !important;
    }
  `;
  if (effectiveTheme === 'dark') {
    colorsCss = `
      :root { color-scheme: dark; }
      html, body {
        background: #141821 !important;
        color: #eef2f8 !important;
      }
      a { color: #7aa2f7 !important; }
    `;
  } else if (effectiveTheme === 'sepia') {
    colorsCss = `
      :root { color-scheme: light; }
      html, body {
        background: #f5ecd7 !important;
        color: #3c2f20 !important;
      }
      a { color: #8c5220 !important; }
    `;
  }

  return `
    ${colorsCss}
    html, body {
      font-size: ${fontSizePx}px !important;
      line-height: 1.65 !important;
      text-align: justify;
      margin: 0 !important;
    }
    p {
      margin: 0 0 0.75em 0;
    }
  `;
}

export async function mountEpubReader({
  book,
  initialPosition,
  settings,
  viewportEl,
  statusLabelEl,
  tocListEl,
  onSavePosition,
}) {
  viewportEl.innerHTML = '<div class="reader-loading">Carregando EPUB...</div>';

  let rawSetting = Number(settings?.default_font_size) || 18;
  let basePx = rawSetting > 48 ? Math.round((rawSetting / 100) * 16) : rawSetting;
  basePx = Math.max(12, Math.min(36, basePx));
  let currentTheme = settings?.theme || 'auto';

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
  view.renderer?.setStyles?.(buildEpubStyles(basePx, currentTheme));

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

  view.addEventListener('relocate', (ev) => {
    if (destroyed) return;
    const d = ev.detail || {};
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

    if (statusLabelEl) {
      const chap = currentSnippet ? ` · ${currentSnippet}` : '';
      statusLabelEl.textContent = `${formatPercent(currentPercent)}${chap}`;
    }

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
    changeFontSize(deltaStep) {
      const stepPx = deltaStep > 0 ? 2 : -2;
      basePx = Math.max(12, Math.min(36, basePx + stepPx));
      view.renderer?.setStyles?.(buildEpubStyles(basePx, currentTheme));
      api.saveSettings({ defaultFontSize: basePx }).catch(() => {});
      return basePx;
    },
    setTheme(theme) {
      currentTheme = theme;
      view.renderer?.setStyles?.(buildEpubStyles(basePx, currentTheme));
    },
    async goToRemotePosition(remotePos) {
      if (!remotePos || !isEpubCfi(remotePos.locator)) return;
      suppressSave = true;
      debouncer.cancel();
      try {
        await view.goTo(remotePos.locator);
        currentLocator = remotePos.locator;
        currentPercent = Number(remotePos.percent) || currentPercent;
        if (statusLabelEl) {
          statusLabelEl.textContent = `${formatPercent(currentPercent)}${
            remotePos.snippet ? ` · ${remotePos.snippet}` : ''
          }`;
        }
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
      await debouncer.flush();
      try {
        view.close?.();
      } catch (_) {}
      viewportEl.innerHTML = '';
    },
  };
}
