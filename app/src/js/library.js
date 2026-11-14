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
  createIcon,
  downloadModal,
  resolveDeviceLabel,
  showToast,
} from './ui.js';

/** "há 5 min", "há 3 h", "ontem", "há 4 dias", "12 de mar." — para a última leitura. */
function formatLastRead(ms) {
  const t = Number(ms) || 0;
  if (t <= 0) return '';
  const diff = Math.max(0, Date.now() - t);
  const min = Math.round(diff / 60000);
  if (min < 1) return 'agora há pouco';
  if (min < 60) return `há ${min} min`;
  const h = Math.round(min / 60);
  if (h < 24) return `há ${h} h`;
  const d = Math.round(h / 24);
  if (d === 1) return 'ontem';
  if (d < 7) return `há ${d} dias`;
  return new Date(t).toLocaleDateString('pt-BR', { day: 'numeric', month: 'short' });
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/**
 * Cria o elemento DOM de um card de livro da biblioteca.
 * Na grade, a capa (2:3) é o card; sem imagem, vira uma "edição de bolso" com o título.
 * Em "Continuar lendo" o card é horizontal: capa pequena, título, progresso e última leitura.
 * @param {object} book
 * @param {(book: object) => void} onOpenBook
 * @param {(book: object) => void} onRemoveDownload
 * @returns {HTMLElement}
 */
function createBookCardElement(book, onOpenBook, onRemoveDownload, { inContinue = false } = {}) {
  const card = document.createElement('article');
  card.className = inContinue ? 'book-card book-card--continue' : 'book-card';
  card.dataset.bookId = book.book_id;
  card.dataset.format = book.format;

  const name = bookDisplayTitle(book);
  const cover = deriveCoverStyle(book.book_id, name, book.format);
  const pct = book.position ? Number(book.position.percent) || 0 : 0;
  const pctLabel = formatPercent(pct);
  const done = pct >= 99.5;

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

  // Capa: imagem (quando houver) sobre uma capa gerada com cor derivada do book_id
  const coverDiv = el('div', 'book-cover');
  coverDiv.style.background = cover.bgGradient;

  const art = el('div', 'book-cover-art');
  art.append(el('span', 'book-cover-rule'), el('span', 'book-cover-title', name));

  const formatBadge = el('span', `format-badge format-${String(book.format).toLowerCase()}`, cover.badge);

  const dlBadge = el('span', `dl-badge ${book.downloaded ? 'is-downloaded' : 'is-cloud'}`);
  dlBadge.title = book.downloaded
    ? 'Baixado neste aparelho'
    : 'Disponível no servidor (nuvem local)';
  dlBadge.setAttribute('aria-label', dlBadge.title);
  dlBadge.appendChild(createIcon(book.downloaded ? 'i-check' : 'i-cloud'));

  const initialSpan = el('span', 'book-cover-initial', cover.initial);
  initialSpan.setAttribute('aria-hidden', 'true');

  coverDiv.append(art, initialSpan, formatBadge, dlBadge);
  if (pct > 0 && !done) {
    const coverProgress = el('div', 'book-cover-progress');
    const coverFill = document.createElement('span');
    coverFill.style.width = `${Math.min(100, Math.max(0, pct))}%`;
    coverProgress.appendChild(coverFill);
    coverDiv.appendChild(coverProgress);
  }
  attachCover(coverDiv, book);

  // Corpo do card
  const bodyDiv = el('div', 'book-card-body');

  const titleH3 = el('h3', 'book-title', name);
  titleH3.title = book.display_title ? `${name} (arquivo: ${book.title})` : name;

  const metaDiv = el('div', 'book-meta');
  const pctSpan = el('span', 'book-pct-label');
  if (pct <= 0) {
    pctSpan.textContent = 'Novo';
    pctSpan.classList.add('is-new');
  } else if (done) {
    pctSpan.textContent = 'Lido';
    pctSpan.classList.add('is-done');
  } else {
    pctSpan.textContent = inContinue ? `${pctLabel} lido` : pctLabel;
  }
  metaDiv.appendChild(pctSpan);
  if (inContinue) {
    const when = formatLastRead(book.position?.updated_at_ms);
    if (when) metaDiv.appendChild(el('span', 'book-last-read', when));
  } else {
    metaDiv.appendChild(el('span', 'book-size', formatBytes(book.size_bytes ?? book.size ?? 0)));
  }

  bodyDiv.append(titleH3, metaDiv);

  if (inContinue) {
    const progressTrack = el('div', 'progress-track');
    const progressFill = el('div', 'progress-fill');
    progressFill.style.width = `${Math.min(100, Math.max(0, pct))}%`;
    progressTrack.appendChild(progressFill);
    const cta = el('span', 'book-continue-cta', 'Continuar');
    cta.appendChild(createIcon('i-chevron-right'));
    bodyDiv.append(progressTrack, cta);
  }

  openBtn.append(coverDiv, bodyDiv);
  card.appendChild(openBtn);

  // Ações rápidas: tirar do "Continuar lendo" (só nessa seção), remover cópia local e menu.
  const actions = el('div', 'book-card-actions');
  const action = (cls, icon, title, onClick) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = `book-card-action ${cls}`;
    b.title = title;
    b.appendChild(createIcon(icon));
    b.addEventListener('click', (ev) => {
      ev.stopPropagation();
      onClick();
    });
    return b;
  };

  const menuBtn = action('book-menu-btn', 'i-more', 'Editar nome e capa', () => openBookMenu(book));
  menuBtn.setAttribute('aria-label', `Editar nome e capa de ${name}`);
  actions.appendChild(menuBtn);

  if (inContinue) {
    const hideBtn = action('book-continue-hide-btn', 'i-x', 'Tirar do Continuar lendo', async () => {
      try {
        await api.setContinueHidden(book.book_id, true);
        showToast('Tirado do Continuar lendo. Volta sozinho quando você ler de novo.');
      } catch (err) {
        showToast(`Não foi possível tirar do Continuar lendo: ${err}`);
      }
    });
    hideBtn.setAttribute('aria-label', `Tirar ${name} do Continuar lendo`);
    actions.appendChild(hideBtn);
  } else if (book.downloaded) {
    const removeBtn = action(
      'book-remove-dl-btn',
      'i-trash',
      'Remover cópia baixada deste aparelho',
      () => onRemoveDownload(book),
    );
    removeBtn.setAttribute('aria-label', `Remover download de ${name}`);
    actions.appendChild(removeBtn);
  }
  card.appendChild(actions);

  // Botão direito (desktop) e toque longo (celular) também abrem o menu.
  card.addEventListener('contextmenu', (ev) => {
    ev.preventDefault();
    openBookMenu(book);
  });

  return card;
}

// Pasta atualmente aberta na biblioteca (`""` = raiz) e ultimos argumentos de renderizacao,
// para re-renderizar ao navegar sem mexer nos chamadores.
let currentFolder = '';
let lastRender = null;

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
  icon.appendChild(createIcon('i-folder'));
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
  const pageTitle = document.getElementById('library-page-title');
  if (pageTitle) {
    const crumbs = folderBreadcrumbs(currentFolder);
    pageTitle.textContent =
      !searching && crumbs.length > 0 ? crumbs[crumbs.length - 1].name : 'Biblioteca';
  }

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

  let cls = 'sync-offline';
  let text = isStale ? 'Offline · cache local' : 'Offline';
  if (syncState === 'syncing') {
    cls = 'sync-syncing';
    text = 'Sincronizando…';
  } else if (syncState === 'connected' && !isStale) {
    cls = 'sync-online';
    text = 'Conectado';
  }
  pill.className = `sync-pill ${cls}`;
  pill.title = text;
  pill.replaceChildren(el('span', 'sync-dot'), el('span', 'sync-text', text));
  pill.firstChild.setAttribute('aria-hidden', 'true');
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
