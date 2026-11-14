// Gerenciamento da Biblioteca em grade, seção "Continuar lendo", busca,
// download com progresso/cancelamento e fluxo de retomada (`decide_book_resume`).

import { api } from './api.js';
import { attachCover } from './covers.js';
import { openBookMenu } from './book-menu.js';
import {
  bookDisplayTitle,
  buildFolderView,
  deriveCoverStyle,
  filterAndSortLibrary,
  folderBreadcrumbs,
  formatBytes,
  formatPercent,
  normalizeFolderPath,
  resolveExistingFolder,
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
function createBookCardElement(book, onOpenBook, onRemoveDownload, { inContinue = false } = {}) {
  const card = document.createElement('article');
  card.className = 'book-card';
  card.dataset.bookId = book.book_id;
  card.dataset.format = book.format;

  const name = bookDisplayTitle(book);
  const cover = deriveCoverStyle(book.book_id, name, book.format);
  const pct = book.position ? Number(book.position.percent) || 0 : 0;
  const pctLabel = formatPercent(pct);

  const openBtn = document.createElement('button');
  openBtn.type = 'button';
  openBtn.className = 'book-card-main';
  openBtn.setAttribute(
    'aria-label',
    `Abrir ${name} (${cover.badge}, ${pctLabel} lido, ${
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
  attachCover(coverDiv, book);

  // Corpo do card
  const bodyDiv = document.createElement('div');
  bodyDiv.className = 'book-card-body';

  const titleH3 = document.createElement('h3');
  titleH3.className = 'book-title';
  titleH3.title = book.display_title ? `${name} (arquivo: ${book.title})` : name;
  titleH3.textContent = name;

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

  // Ações rápidas sobre a capa: tirar do "Continuar lendo" (só nessa seção) e menu do livro.
  const actions = document.createElement('div');
  actions.className = 'book-card-actions';
  if (inContinue) {
    const hideBtn = document.createElement('button');
    hideBtn.type = 'button';
    hideBtn.className = 'book-card-action book-continue-hide-btn';
    hideBtn.title = 'Tirar do Continuar lendo';
    hideBtn.setAttribute('aria-label', `Tirar ${name} do Continuar lendo`);
    hideBtn.textContent = '✕';
    hideBtn.addEventListener('click', async (ev) => {
      ev.stopPropagation();
      try {
        await api.setContinueHidden(book.book_id, true);
        showToast('Tirado do Continuar lendo. Volta sozinho quando você ler de novo.');
      } catch (err) {
        showToast(`Não foi possível tirar do Continuar lendo: ${err}`);
      }
    });
    actions.appendChild(hideBtn);
  }
  const menuBtn = document.createElement('button');
  menuBtn.type = 'button';
  menuBtn.className = 'book-card-action book-menu-btn';
  menuBtn.title = 'Editar nome e capa';
  menuBtn.setAttribute('aria-label', `Editar nome e capa de ${name}`);
  menuBtn.textContent = '⋯';
  menuBtn.addEventListener('click', (ev) => {
    ev.stopPropagation();
    openBookMenu(book);
  });
  actions.appendChild(menuBtn);
  card.appendChild(actions);
  // Botão direito (desktop) e toque longo (celular) também abrem o menu.
  card.addEventListener('contextmenu', (ev) => {
    ev.preventDefault();
    openBookMenu(book);
  });

  if (book.downloaded) {
    const removeBtn = document.createElement('button');
    removeBtn.type = 'button';
    removeBtn.className = 'book-remove-dl-btn';
    removeBtn.title = 'Remover cópia baixada deste aparelho';
    removeBtn.setAttribute('aria-label', `Remover download de ${name}`);
    removeBtn.textContent = 'Remover cópia local';
    removeBtn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      onRemoveDownload(book);
    });
    card.appendChild(removeBtn);
  }

  return card;
}

// Pasta atualmente aberta na biblioteca (`""` = raiz) e ultimos argumentos de renderizacao,
// para re-renderizar ao navegar sem mexer nos chamadores.
let currentFolder = '';
let lastRender = null;

const FOLDER_ICON_SVG =
  '<svg viewBox="0 0 24 24" width="40" height="40" aria-hidden="true" focusable="false">' +
  '<path fill="currentColor" d="M3 6.5A2.5 2.5 0 0 1 5.5 4h4.3c.6 0 1.2.3 1.6.8L12.8 6.5H18.5A2.5 2.5 0 0 1 21 9v8.5a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"/></svg>';

function createFolderCardElement(folder, onEnter) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'folder-card';
  btn.dataset.folderPath = folder.path;
  btn.setAttribute(
    'aria-label',
    `Abrir pasta ${folder.name} (${folder.count} ${folder.count === 1 ? 'livro' : 'livros'})`,
  );
  const icon = document.createElement('span');
  icon.className = 'folder-card-icon';
  icon.innerHTML = FOLDER_ICON_SVG; // constante estatica, sem dados do servidor
  const name = document.createElement('span');
  name.className = 'folder-card-name';
  name.textContent = folder.name;
  name.title = folder.name;
  const count = document.createElement('span');
  count.className = 'folder-card-count';
  count.textContent = folder.count === 1 ? '1 livro' : `${folder.count} livros`;
  btn.append(icon, name, count);
  btn.addEventListener('click', () => onEnter(folder.path));
  return btn;
}

function renderBreadcrumb(path) {
  const nav = document.getElementById('folder-breadcrumb');
  if (!nav) return;
  nav.innerHTML = '';
  if (path === '') {
    nav.classList.add('hidden');
    return;
  }
  nav.classList.remove('hidden');
  const crumbs = [{ name: 'Biblioteca', path: '' }, ...folderBreadcrumbs(path)];
  crumbs.forEach((c, i) => {
    const last = i === crumbs.length - 1;
    const el = document.createElement(last ? 'span' : 'button');
    el.className = last ? 'crumb crumb-current' : 'crumb';
    el.textContent = c.name;
    if (last) {
      el.setAttribute('aria-current', 'page');
    } else {
      el.type = 'button';
      el.addEventListener('click', () => navigateToFolder(c.path));
    }
    nav.appendChild(el);
    if (!last) {
      const sep = document.createElement('span');
      sep.className = 'crumb-sep';
      sep.setAttribute('aria-hidden', 'true');
      sep.textContent = '/';
      nav.appendChild(sep);
    }
  });
}

/**
 * Abre uma pasta da biblioteca. Empilha no historico do webview para o botao "voltar"
 * (Android) subir um nivel em vez de fechar o app.
 * @param {string} path
 * @param {{ push?: boolean }} [opts]
 */
export function navigateToFolder(path, { push = true } = {}) {
  const target = normalizeFolderPath(path);
  if (target === currentFolder) return;
  currentFolder = target;
  if (push && typeof history !== 'undefined') {
    history.pushState({ bookferryFolder: target }, '');
  }
  if (lastRender) {
    const { books, searchQuery, onOpenBook, onRemoveDownload } = lastRender;
    renderLibraryView(books, searchQuery, onOpenBook, onRemoveDownload);
    document.getElementById('library-scroll-area')?.scrollTo?.({ top: 0 });
  }
}

if (typeof window !== 'undefined') {
  window.addEventListener('popstate', (ev) => {
    navigateToFolder(ev.state?.bookferryFolder ?? '', { push: false });
  });
}

/**
 * Renderiza a biblioteca: navegação por pastas, seção "Continuar lendo" (só na raiz) e grade.
 * Com busca ativa, procura em TODAS as pastas (lista plana).
 * @param {Array<object>} books
 * @param {string} searchQuery
 * @param {(book: object) => void} onOpenBook
 * @param {(book: object) => void} onRemoveDownload
 */
export function renderLibraryView(books, searchQuery, onOpenBook, onRemoveDownload) {
  lastRender = { books, searchQuery, onOpenBook, onRemoveDownload };
  const continueSection = document.getElementById('continue-reading-section');
  const continueGrid = document.getElementById('continue-reading-grid');
  const mainGrid = document.getElementById('library-grid');
  const folderGrid = document.getElementById('folder-grid');
  const foldersSection = document.getElementById('folders-section');
  const booksTitle = document.getElementById('books-section-title');
  const emptyState = document.getElementById('library-empty-state');
  const countLabel = document.getElementById('library-count-label');

  if (!mainGrid) return;

  const searching = String(searchQuery || '').trim() !== '';
  currentFolder = resolveExistingFolder(books, currentFolder);
  const { continueReading, filteredBooks } = filterAndSortLibrary(books, searchQuery);
  const view = searching
    ? { folders: [], books: filteredBooks }
    : buildFolderView(books, currentFolder);

  if (countLabel) {
    const nb = view.books.length;
    const nf = view.folders.length;
    const parts = [];
    if (nf > 0) parts.push(nf === 1 ? '1 pasta' : `${nf} pastas`);
    if (nb > 0 || nf === 0) parts.push(nb === 1 ? '1 livro' : `${nb} livros`);
    countLabel.textContent = parts.join(' · ');
  }

  renderBreadcrumb(searching ? '' : currentFolder);

  // Seção "Continuar lendo": só na raiz (ou nos resultados de busca)
  if (continueSection && continueGrid) {
    continueGrid.innerHTML = '';
    if ((searching || currentFolder === '') && continueReading.length > 0) {
      continueSection.classList.remove('hidden');
      for (const b of continueReading.slice(0, 4)) {
        continueGrid.appendChild(
          createBookCardElement(b, onOpenBook, onRemoveDownload, { inContinue: true }),
        );
      }
    } else {
      continueSection.classList.add('hidden');
    }
  }

  // Pastas (só as que têm algum livro dentro, por construção)
  if (folderGrid && foldersSection) {
    folderGrid.innerHTML = '';
    if (view.folders.length > 0) {
      foldersSection.classList.remove('hidden');
      for (const f of view.folders) {
        folderGrid.appendChild(createFolderCardElement(f, (p) => navigateToFolder(p)));
      }
    } else {
      foldersSection.classList.add('hidden');
    }
  }

  // Livros da pasta atual
  mainGrid.innerHTML = '';
  const hasContent = view.folders.length > 0 || view.books.length > 0;
  if (booksTitle) {
    booksTitle.parentElement?.classList.toggle('hidden', view.books.length === 0);
    booksTitle.textContent = searching ? 'Resultados' : 'Livros';
  }
  if (!hasContent) {
    if (emptyState) emptyState.classList.remove('hidden');
  } else {
    if (emptyState) emptyState.classList.add('hidden');
    for (const b of view.books) {
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
