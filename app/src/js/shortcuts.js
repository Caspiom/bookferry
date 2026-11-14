// Ajuda de atalhos (tecla ? ou F1) e diálogo "Ir para..." (tecla G).
// A decisão de qual tecla faz o quê fica em state.js; aqui só há DOM.

import { parseGotoTarget } from './state.js';

const $ = (id) => document.getElementById(id);

/** Grupos mostrados na ajuda. `keys`: alternativas; cada item é uma tecla (ou combinação). */
export const SHORTCUT_GROUPS = [
  {
    title: 'Leitor — navegar',
    rows: [
      { what: 'Próxima página', keys: ['→', '↓', 'PgDn', 'Espaço', 'N', 'Roda para baixo'] },
      { what: 'Página anterior', keys: ['←', '↑', 'PgUp', 'Shift+Espaço', 'P', 'Roda para cima'] },
      { what: 'Primeira / última página', keys: ['Home', 'End'] },
      { what: 'Ir para página (ou %)', keys: ['G', 'Ctrl+G'] },
      { what: 'Sumário', keys: ['T'] },
    ],
  },
  {
    title: 'Leitor — zoom e vista',
    rows: [
      { what: 'Ampliar', keys: ['+', 'Ctrl++', 'Ctrl+Roda para cima'] },
      { what: 'Reduzir', keys: ['−', 'Ctrl+−', 'Ctrl+Roda para baixo'] },
      { what: 'Zoom padrão (100%)', keys: ['0', 'Ctrl+0'] },
      { what: 'Encaixe: página / largura / altura', keys: ['W'] },
      { what: 'Esconder barras (modo imersivo)', keys: ['F'] },
      { what: 'Tela cheia', keys: ['F11'] },
      { what: 'Voltar: fecha painel, tela cheia, imersivo, leitor', keys: ['Esc'] },
    ],
  },
  {
    title: 'Biblioteca',
    rows: [
      { what: 'Buscar', keys: ['/', 'Ctrl+F'] },
      { what: 'Atualizar', keys: ['F5', 'Ctrl+R'] },
      { what: 'Ajustes', keys: ['Ctrl+,'] },
      { what: 'Subir uma pasta', keys: ['Backspace', 'Alt+←'] },
      { what: 'Esta ajuda', keys: ['?', 'F1'] },
    ],
  },
];

const NOTES = [
  'Com a página ampliada, as setas e a roda movem a página; no limite, viram a página.',
  'Em quadrinhos de mangá (direita para esquerda), ← avança e → volta.',
  'No EPUB, + e − mudam o tamanho da letra e 0 volta ao padrão; G vai a um percentual do livro.',
  'No Mac, Cmd faz o papel do Ctrl na roda do mouse.',
];

function renderKeys(keys) {
  const frag = document.createDocumentFragment();
  keys.forEach((k, i) => {
    // O "ou" e a tecla seguinte formam um bloco que não se separa ao quebrar a linha.
    const alt = document.createElement('span');
    alt.className = 'sc-alt';
    if (i > 0) {
      const or = document.createElement('span');
      or.className = 'sc-or';
      or.textContent = 'ou';
      alt.appendChild(or);
    }
    // Ações do mouse não são teclas: sem o visual de tecla.
    const isMouse = /roda/i.test(k);
    const el = document.createElement(isMouse ? 'span' : 'kbd');
    if (isMouse) el.className = 'sc-mouse';
    el.textContent = k;
    alt.appendChild(el);
    frag.appendChild(alt);
  });
  return frag;
}

function buildHelp() {
  const body = $('shortcuts-body');
  if (!body || body.dataset.built) return;
  body.dataset.built = '1';
  for (const g of SHORTCUT_GROUPS) {
    const sec = document.createElement('section');
    sec.className = 'shortcuts-group';
    const h3 = document.createElement('h3');
    h3.textContent = g.title;
    sec.appendChild(h3);
    for (const row of g.rows) {
      const div = document.createElement('div');
      div.className = 'shortcuts-row';
      const what = document.createElement('span');
      what.textContent = row.what;
      const keys = document.createElement('span');
      keys.className = 'shortcuts-keys';
      keys.appendChild(renderKeys(row.keys));
      div.append(what, keys);
      sec.appendChild(div);
    }
    body.appendChild(sec);
  }
  for (const n of NOTES) {
    const p = document.createElement('p');
    p.className = 'muted-note shortcuts-note';
    p.textContent = n;
    body.appendChild(p);
  }
}

/**
 * @param {{ getReader: () => object | null }} deps
 */
export function mountShortcutsUi({ getReader }) {
  const help = $('shortcuts-modal');
  const gotoModal = $('goto-modal');
  const gotoInput = $('goto-input');
  let returnFocus = null;

  function openHelp() {
    if (!help) return;
    buildHelp();
    returnFocus = document.activeElement;
    help.classList.remove('hidden');
    $('shortcuts-close')?.focus();
  }

  function closeHelp() {
    if (!help || help.classList.contains('hidden')) return;
    help.classList.add('hidden');
    returnFocus?.focus?.();
  }

  /** PDF/quadrinhos: número da página; EPUB: percentual do livro. */
  function gotoMode(reader) {
    return reader?.seekPage ? 'page' : reader?.seekFraction ? 'percent' : null;
  }

  function openGoto() {
    const reader = getReader();
    const mode = gotoMode(reader);
    if (!gotoModal || !mode) return;
    returnFocus = document.activeElement;
    $('goto-title').textContent = mode === 'page' ? 'Ir para a página' : 'Ir para o percentual';
    $('goto-hint').textContent =
      mode === 'page' ? 'Digite o número da página e tecle Enter.' : 'Digite de 0 a 100 e tecle Enter.';
    gotoInput.value = '';
    gotoInput.placeholder = mode === 'page' ? 'Ex.: 42' : 'Ex.: 50';
    gotoModal.classList.remove('hidden');
    gotoInput.focus();
  }

  function closeGoto() {
    if (!gotoModal || gotoModal.classList.contains('hidden')) return;
    gotoModal.classList.add('hidden');
    returnFocus?.focus?.();
  }

  $('goto-form')?.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const reader = getReader();
    const mode = gotoMode(reader);
    if (!mode) return closeGoto();
    // Páginas: o leitor limita ao total; EPUB: percentual de 0 a 100.
    const n = mode === 'page' ? parseGotoTarget(gotoInput.value, 9999999) : parseGotoTarget(gotoInput.value, 100, 0);
    if (n === null) {
      $('goto-hint').textContent = 'Número inválido. Digite só dígitos.';
      gotoInput.select();
      return;
    }
    closeGoto();
    if (mode === 'page') reader.seekPage(n);
    else reader.seekFraction(n / 100);
  });
  $('goto-cancel')?.addEventListener('click', closeGoto);
  $('shortcuts-close')?.addEventListener('click', closeHelp);
  for (const m of [help, gotoModal]) {
    m?.addEventListener('click', (ev) => {
      if (ev.target === m) (m === help ? closeHelp : closeGoto)();
    });
  }

  /**
   * Captura as teclas enquanto uma destas janelas está aberta. Devolve true se tratou o evento
   * (quem chama não deve repassá-lo ao leitor/biblioteca).
   * @param {KeyboardEvent} ev
   */
  function handleKey(ev) {
    const helpOpen = help && !help.classList.contains('hidden');
    const gotoOpen = gotoModal && !gotoModal.classList.contains('hidden');
    if (!helpOpen && !gotoOpen) return false;
    if (ev.key === 'Escape' || (helpOpen && (ev.key === '?' || ev.key === 'F1'))) {
      ev.preventDefault();
      if (helpOpen) closeHelp();
      else closeGoto();
    }
    return true; // com a janela aberta, o resto das teclas não chega ao livro
  }

  return {
    openHelp,
    openGoto,
    handleKey,
    isOpen: () =>
      Boolean(
        (help && !help.classList.contains('hidden')) ||
          (gotoModal && !gotoModal.classList.contains('hidden')),
      ),
  };
}
