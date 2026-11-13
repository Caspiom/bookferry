// Gerenciamento da Biblioteca em grade, seção "Continuar lendo", busca,
// download com progresso/cancelamento e fluxo de retomada (`decide_book_resume`).

import { api } from './api.js';
import {
  deriveCoverStyle,
  filterAndSortLibrary,
  formatBytes,
  formatPercent,
} from './state.js';
import {
  askResumeModal,
  downloadModal,
  resolveDeviceLabel,
  showToast,
} from './ui.js';

/**
 * Cria o elemento DOM de um card de livro da biblioteca.
 * @param {object} book
 * @param {(book: object) => void} onOpenBook
 * @param {(book: object) => void} onRemoveDownload
 * @returns {HTMLElement}
 */
function createBookCardElement(book, onOpenBook, onRemoveDownload) {
  const card = document.createElement('article');
  card.className = 'book-card';
  card.dataset.bookId = book.book_id;
  card.dataset.format = book.format;

  const cover = deriveCoverStyle(book.book_id, book.title, book.format);
  const pct = book.position ? Number(book.position.percent) || 0 : 0;
  const pctLabel = formatPercent(pct);

  const openBtn = document.createElement('button');
  openBtn.type = 'button';
  openBtn.className = 'book-card-main';
  openBtn.setAttribute(
    'aria-label',
    `Abrir ${book.title} (${cover.badge}, ${pctLabel} lido, ${
      book.downloaded ? 'baixado' : 'na nuvem'
    })`,
  );
  openBtn.addEventListener('click', () => onOpenBook(book));

  // Capa placeholder gerada (inicial + cor derivada de book_id + selo do formato)
  const coverDiv = document.createElement('div');
  coverDiv.className = 'book-cover';
  coverDiv.style.background = cover.bgGradient;

  const formatBadge = document.createElement('span');
  formatBadge.className = `format-badge format-${String(book.format).toLowerCase()}`;
  formatBadge.textContent = cover.badge;

  const dlBadge = document.createElement('span');
  dlBadge.className = `dl-badge ${book.downloaded ? 'is-downloaded' : 'is-cloud'}`;
  dlBadge.title = book.downloaded
    ? 'Baixado neste aparelho'
    : 'Disponível no servidor (nuvem local)';
  dlBadge.setAttribute('aria-label', dlBadge.title);
  dlBadge.textContent = book.downloaded ? '✓ Baixado' : '☁ Nuvem';

  const initialSpan = document.createElement('span');
  initialSpan.className = 'book-cover-initial';
  initialSpan.textContent = cover.initial;

  coverDiv.appendChild(formatBadge);
  coverDiv.appendChild(dlBadge);
  coverDiv.appendChild(initialSpan);

  // Corpo do card
  const bodyDiv = document.createElement('div');
  bodyDiv.className = 'book-card-body';

  const titleH3 = document.createElement('h3');
  titleH3.className = 'book-title';
  titleH3.title = book.title;
  titleH3.textContent = book.title;

  const metaDiv = document.createElement('div');
  metaDiv.className = 'book-meta';
  const sizeSpan = document.createElement('span');
  sizeSpan.textContent = formatBytes(book.size_bytes ?? book.size ?? 0);
  const pctSpan = document.createElement('span');
  pctSpan.className = 'book-pct-label';
  pctSpan.textContent = pct > 0 ? pctLabel : 'Novo';
  metaDiv.appendChild(sizeSpan);
  metaDiv.appendChild(pctSpan);

  const progressTrack = document.createElement('div');
  progressTrack.className = 'progress-track';
  const progressFill = document.createElement('div');
  progressFill.className = 'progress-fill';
  progressFill.style.width = `${Math.min(100, Math.max(0, pct))}%`;
  progressTrack.appendChild(progressFill);

  bodyDiv.appendChild(titleH3);
  bodyDiv.appendChild(metaDiv);
  bodyDiv.appendChild(progressTrack);

  openBtn.appendChild(coverDiv);
  openBtn.appendChild(bodyDiv);
  card.appendChild(openBtn);

  if (book.downloaded) {
    const removeBtn = document.createElement('button');
    removeBtn.type = 'button';
    removeBtn.className = 'book-remove-dl-btn';
    removeBtn.title = 'Remover cópia baixada deste aparelho';
    removeBtn.setAttribute('aria-label', `Remover download de ${book.title}`);
    removeBtn.textContent = 'Remover cópia local';
    removeBtn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      onRemoveDownload(book);
    });
    card.appendChild(removeBtn);
  }

  return card;
}

/**
 * Renderiza a seção "Continuar lendo" e a grade completa da biblioteca.
 * @param {Array<object>} books
 * @param {string} searchQuery
 * @param {(book: object) => void} onOpenBook
 * @param {(book: object) => void} onRemoveDownload
 */
export function renderLibraryView(books, searchQuery, onOpenBook, onRemoveDownload) {
  const continueSection = document.getElementById('continue-reading-section');
  const continueGrid = document.getElementById('continue-reading-grid');
  const mainGrid = document.getElementById('library-grid');
  const emptyState = document.getElementById('library-empty-state');
  const countLabel = document.getElementById('library-count-label');

  if (!mainGrid) return;

  const { continueReading, filteredBooks } = filterAndSortLibrary(books, searchQuery);

  if (countLabel) {
    countLabel.textContent =
      filteredBooks.length === 1 ? '1 livro' : `${filteredBooks.length} livros`;
  }

  // Seção "Continuar lendo"
  if (continueSection && continueGrid) {
    continueGrid.innerHTML = '';
    if (continueReading.length > 0) {
      continueSection.classList.remove('hidden');
      for (const b of continueReading.slice(0, 4)) {
        continueGrid.appendChild(createBookCardElement(b, onOpenBook, onRemoveDownload));
      }
    } else {
      continueSection.classList.add('hidden');
    }
  }

  // Grade principal
  mainGrid.innerHTML = '';
  if (filteredBooks.length === 0) {
    if (emptyState) emptyState.classList.remove('hidden');
  } else {
    if (emptyState) emptyState.classList.add('hidden');
    for (const b of filteredBooks) {
      mainGrid.appendChild(createBookCardElement(b, onOpenBook, onRemoveDownload));
    }
  }
}

/**
 * Atualiza o indicador visual de sincronização (conectado / sincronizando / offline stale).
 * @param {'connected'|'syncing'|'offline'} syncState
 * @param {boolean} isStale
 */
export function updateSyncIndicator(syncState, isStale = false) {
  const pill = document.getElementById('sync-status-pill');
  if (!pill) return;

  if (syncState === 'syncing') {
    pill.className = 'sync-pill sync-syncing';
    pill.textContent = '↻ Sincronizando…';
  } else if (syncState === 'connected' && !isStale) {
    pill.className = 'sync-pill sync-online';
    pill.textContent = '● Conectado';
  } else {
    pill.className = 'sync-pill sync-offline';
    pill.textContent = isStale ? '○ Offline · cache local' : '○ Offline';
  }
}

/**
 * Executa o fluxo completo de abertura de um livro:
 * 1. Baixa se necessário (com modal de progresso e botão cancelar).
 * 2. Consulta `decide_book_resume(book_id)`.
 * 3. Trata `Ask` (modal), `UseRemote` (toast discreto), `UseLocal` e `Nothing`.
 * @param {object} book
 * @returns {Promise<{ book: object, initialPosition: object|null }|null>}
 */
export async function prepareBookToOpen(book) {
  let updatedBook = { ...book };

  if (!updatedBook.downloaded) {
    let cancelled = false;
    downloadModal.show(updatedBook.title, async () => {
      cancelled = true;
      try {
        await api.cancelDownload(updatedBook.book_id);
      } catch (_) {}
      downloadModal.hide();
      showToast('Download cancelado.');
    });

    try {
      await api.downloadBook(updatedBook.book_id);
      downloadModal.hide();
      if (cancelled) return null;
      updatedBook.downloaded = true;
    } catch (err) {
      downloadModal.hide();
      if (!cancelled) {
        showToast(`Falha ao baixar livro: ${err}`);
      }
      return null;
    }
  }

  let initialPosition = updatedBook.position || null;
  try {
    const decision = await api.decideBookResume(updatedBook.book_id);
    if (decision && decision.decision === 'ask' && decision.remote) {
      const choice = await askResumeModal(decision.remote, decision.local);
      if (choice === 'remote') {
        initialPosition = decision.remote;
      } else {
        initialPosition = decision.local;
      }
    } else if (decision && decision.decision === 'use_remote') {
      const rem = decision.remote || decision.position;
      if (rem) {
        initialPosition = rem;
        const dev = resolveDeviceLabel(rem);
        const pct = formatPercent(rem.percent);
        showToast(`Retomando de onde parou em ${dev} (${pct})`);
      }
    } else if (decision && decision.decision === 'use_local') {
      initialPosition = decision.local || decision.position || initialPosition;
    } else if (decision && decision.decision === 'nothing') {
      initialPosition = null;
    }
  } catch (_) {
    // Se a decisão falhar por algum motivo inesperado, segue com a posição local conhecida
  }

  return { book: updatedBook, initialPosition };
}
