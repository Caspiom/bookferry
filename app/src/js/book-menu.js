// Modal "Editar livro": nome de exibição, capa (imagem própria, internet ou original) e
// "Continuar lendo". As escritas vão ao servidor pelo cliente; a tela se atualiza pelo evento
// `BookMetaChanged` (o mesmo que chega dos outros aparelhos), então nada é duplicado aqui.

import { api } from './api.js';
import { resizeImageToJpeg, attachCover } from './covers.js';
import { bookDisplayTitle, deriveCoverStyle, isHiddenFromContinue } from './state.js';
import { showToast } from './ui.js';
import { setupWebCoverSearch } from './web-covers.js';

const $ = (id) => document.getElementById(id);

let current = null; // livro aberto no modal (cópia atualizada por `updateBookMenu`)

function renderPreview() {
  const preview = $('book-menu-cover-preview');
  if (!preview || !current) return;
  const style = deriveCoverStyle(current.book_id, bookDisplayTitle(current), current.format);
  preview.innerHTML = '';
  preview.classList.remove('has-cover-image');
  preview.style.background = style.bgGradient;
  preview.textContent = style.initial;
  if (Number(current.cover_version) > 0) {
    const holder = document.createElement('span');
    holder.className = 'book-cover-initial';
    holder.hidden = true;
    preview.append(holder);
    attachCover(preview, current);
  }
  const reset = $('book-menu-reset-cover');
  if (reset) reset.disabled = Number(current.cover_version) <= 0;
}

function renderFields() {
  if (!current) return;
  const nameInput = $('book-menu-name');
  if (nameInput && document.activeElement !== nameInput) {
    nameInput.value = current.display_title || '';
  }
  if (nameInput) nameInput.placeholder = current.title || 'Nome do arquivo';
  const original = $('book-menu-original');
  if (original) original.textContent = `Arquivo: ${current.title}`;
  const hide = $('book-menu-hide-continue');
  if (hide) {
    const inContinue =
      Number(current.position?.updated_at_ms) > 0 &&
      Number(current.position?.percent) > 0 &&
      !isHiddenFromContinue(current);
    hide.classList.toggle('hidden', !inContinue);
  }
  renderPreview();
}

/** Chamado pelo `main.js` quando os dados do livro mudam enquanto o modal está aberto. */
export function updateBookMenu(book) {
  if (!current || !book || book.book_id !== current.book_id) return;
  current = { ...book };
  renderFields();
}

export function closeBookMenu() {
  $('book-menu-modal')?.classList.add('hidden');
  $('book-menu-web')?.classList.add('hidden');
  current = null;
}

async function guarded(label, fn) {
  try {
    await fn();
  } catch (err) {
    showToast(`${label}: ${err}`);
  }
}

let wired = false;

function wireOnce() {
  if (wired) return;
  wired = true;

  $('book-menu-close')?.addEventListener('click', closeBookMenu);
  $('book-menu-modal')?.addEventListener('click', (ev) => {
    if (ev.target === $('book-menu-modal')) closeBookMenu();
  });
  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && current) closeBookMenu();
  });

  const saveName = () =>
    guarded('Não foi possível salvar o nome', async () => {
      if (!current) return;
      const value = $('book-menu-name').value.trim();
      await api.setDisplayTitle(current.book_id, value === '' ? null : value);
      showToast(value === '' ? 'Nome original restaurado.' : 'Nome salvo.');
    });
  $('book-menu-name-save')?.addEventListener('click', saveName);
  $('book-menu-name')?.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      saveName();
    }
  });

  $('book-menu-pick-image')?.addEventListener('click', () => {
    const input = $('book-menu-file');
    if (input) {
      input.value = '';
      input.click();
    }
  });
  $('book-menu-file')?.addEventListener('change', (ev) =>
    guarded('Não foi possível usar a imagem', async () => {
      const file = ev.target.files?.[0];
      if (!file || !current) return;
      const bytes = await resizeImageToJpeg(file);
      await api.setBookCover(current.book_id, 'custom', bytes);
      showToast('Capa atualizada.');
    }),
  );

  $('book-menu-reset-cover')?.addEventListener('click', () =>
    guarded('Não foi possível restaurar a capa', async () => {
      if (!current) return;
      await api.removeBookCover(current.book_id);
      showToast('Capa original será gerada de novo.');
    }),
  );

  $('book-menu-hide-continue')?.addEventListener('click', () =>
    guarded('Não foi possível tirar do Continuar lendo', async () => {
      if (!current) return;
      await api.setContinueHidden(current.book_id, true);
      showToast('Tirado do Continuar lendo. Volta sozinho quando você ler de novo.');
      closeBookMenu();
    }),
  );

  setupWebCoverSearch({ getBook: () => current });
}

/**
 * Abre o modal para um livro.
 * @param {object} book
 */
export function openBookMenu(book) {
  wireOnce();
  current = { ...book };
  const nameInput = $('book-menu-name');
  if (nameInput) nameInput.value = '';
  $('book-menu-web')?.classList.add('hidden');
  renderFields();
  $('book-menu-modal')?.classList.remove('hidden');
  nameInput?.focus();
}
