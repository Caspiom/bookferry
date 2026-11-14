// Busca de capa na internet (Open Library, sem chave). Só roda quando o usuário aperta o
// botão: nada é consultado em segundo plano. A imagem escolhida é baixada, reduzida aqui e
// enviada ao servidor como capa `web`; os outros aparelhos a recebem por sincronização.

import { api } from './api.js';
import { resizeImageToJpeg } from './covers.js';
import {
  bookDisplayTitle,
  cleanSearchQuery,
  openLibraryCoverUrl,
  openLibrarySearchUrl,
  parseOpenLibraryResults,
} from './state.js';
import { showToast } from './ui.js';

const SEARCH_TIMEOUT_MS = 12000;
const IMAGE_TIMEOUT_MS = 20000;

async function fetchWithTimeout(url, ms) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), ms);
  try {
    return await fetch(url, { signal: ctrl.signal, credentials: 'omit', referrerPolicy: 'no-referrer' });
  } finally {
    clearTimeout(timer);
  }
}

function describeNetworkError(err) {
  if (err?.name === 'AbortError') return 'A busca demorou demais. Tente de novo.';
  return 'Sem conexão com a internet (ou o serviço de capas está fora do ar).';
}

/**
 * @param {{ getBook: () => (object|null) }} deps
 */
export function setupWebCoverSearch({ getBook }) {
  const panel = document.getElementById('book-menu-web');
  const queryInput = document.getElementById('book-menu-web-query');
  const statusEl = document.getElementById('book-menu-web-status');
  const resultsEl = document.getElementById('book-menu-web-results');
  let searchSeq = 0;

  const setStatus = (text) => {
    if (statusEl) statusEl.textContent = text;
  };

  async function runSearch() {
    const query = queryInput?.value.trim() || '';
    if (!query || !resultsEl) return;
    const seq = ++searchSeq;
    resultsEl.innerHTML = '';
    setStatus('Buscando…');
    try {
      const resp = await fetchWithTimeout(openLibrarySearchUrl(query), SEARCH_TIMEOUT_MS);
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const results = parseOpenLibraryResults(await resp.json());
      if (seq !== searchSeq) return; // uma busca mais nova já começou
      if (results.length === 0) {
        setStatus('Nada encontrado. Tente outro texto (por exemplo, só o título).');
        return;
      }
      setStatus('Toque na capa que quiser usar.');
      for (const r of results) {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'book-menu-web-result';
        const label = [r.title, r.author, r.year].filter(Boolean).join(' · ');
        btn.title = label;
        btn.setAttribute('aria-label', `Usar a capa de ${label}`);
        const img = document.createElement('img');
        img.alt = '';
        img.loading = 'lazy';
        img.referrerPolicy = 'no-referrer';
        img.src = openLibraryCoverUrl(r.coverId, 'M');
        img.addEventListener('error', () => btn.remove());
        const cap = document.createElement('span');
        cap.textContent = r.title;
        btn.append(img, cap);
        btn.addEventListener('click', () => choose(r));
        resultsEl.appendChild(btn);
      }
    } catch (err) {
      if (seq === searchSeq) setStatus(describeNetworkError(err));
    }
  }

  async function choose(result) {
    const book = getBook();
    if (!book) return;
    setStatus('Baixando a capa…');
    try {
      const resp = await fetchWithTimeout(openLibraryCoverUrl(result.coverId, 'L'), IMAGE_TIMEOUT_MS);
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const bytes = await resizeImageToJpeg(await resp.blob());
      await api.setBookCover(book.book_id, 'web', bytes);
      showToast('Capa atualizada.');
      panel?.classList.add('hidden');
    } catch (err) {
      setStatus(`Não foi possível usar essa capa: ${err?.name === 'AbortError' ? 'demorou demais' : err}`);
    }
  }

  document.getElementById('book-menu-web-search')?.addEventListener('click', () => {
    const book = getBook();
    if (!book || !panel) return;
    panel.classList.remove('hidden');
    if (queryInput) {
      queryInput.value = cleanSearchQuery(bookDisplayTitle(book)) || cleanSearchQuery(book.title);
    }
    setStatus('');
    if (resultsEl) resultsEl.innerHTML = '';
    runSearch();
  });
  document.getElementById('book-menu-web-go')?.addEventListener('click', runSearch);
  queryInput?.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      runSearch();
    }
  });
}
