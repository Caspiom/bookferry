// Lógica pura de estado, formatação, ordenação natural, locators e debounce do Bookferry.
// Sem dependências de DOM para permitir testes automatizados diretos via `node --test`.

/**
 * Comparação em ordem natural (ex.: "page2.png" < "page10.png"),
 * espelhando a implementação Rust do protocolo CBZ.
 * @param {string} a
 * @param {string} b
 * @returns {number}
 */
export function naturalCompare(a, b) {
  const sA = String(a ?? '');
  const sB = String(b ?? '');
  let i = 0;
  let j = 0;
  while (i < sA.length && j < sB.length) {
    const cA = sA.charCodeAt(i);
    const cB = sB.charCodeAt(j);
    const isDigitA = cA >= 48 && cA <= 57;
    const isDigitB = cB >= 48 && cB <= 57;

    if (isDigitA && isDigitB) {
      let startA = i;
      while (i < sA.length && sA.charCodeAt(i) >= 48 && sA.charCodeAt(i) <= 57) i++;
      let startB = j;
      while (j < sB.length && sB.charCodeAt(j) >= 48 && sB.charCodeAt(j) <= 57) j++;

      const rawA = sA.slice(startA, i);
      const rawB = sB.slice(startB, j);
      const trimA = rawA.replace(/^0+/, '');
      const trimB = rawB.replace(/^0+/, '');

      if (trimA.length !== trimB.length) {
        return trimA.length < trimB.length ? -1 : 1;
      }
      if (trimA !== trimB) {
        return trimA < trimB ? -1 : 1;
      }
      if (rawA.length !== rawB.length) {
        return rawA.length < rawB.length ? -1 : 1;
      }
    } else {
      const lowA = sA[i].toLowerCase();
      const lowB = sB[j].toLowerCase();
      if (lowA !== lowB) {
        return lowA < lowB ? -1 : 1;
      }
      i++;
      j++;
    }
  }
  if (sA.length === sB.length) return 0;
  return sA.length < sB.length ? -1 : 1;
}

/**
 * Converte locator `page:<n>` (1-based) em número de página válido dentro de [1, totalPages].
 * @param {string|null|undefined} locator
 * @param {number} totalPages
 * @returns {number}
 */
export function parsePageLocator(locator, totalPages = 1) {
  const maxPage = Math.max(1, Math.floor(Number(totalPages) || 1));
  if (typeof locator !== 'string') return 1;
  const trimmed = locator.trim();
  const match = /^page:(\d+)$/i.exec(trimmed);
  if (!match) return 1;
  const parsed = Number.parseInt(match[1], 10);
  if (!Number.isFinite(parsed) || parsed < 1) return 1;
  return Math.min(maxPage, parsed);
}

/**
 * Formata número de página 1-based no formato canônico `page:<n>`.
 * @param {number} page
 * @returns {string}
 */
export function formatPageLocator(page) {
  const n = Math.max(1, Math.floor(Number(page) || 1));
  return `page:${n}`;
}

/**
 * Calcula percentual de leitura (0 a 100) a partir da página 1-based e do total de páginas.
 * @param {number} page
 * @param {number} totalPages
 * @returns {number}
 */
export function computePagePercent(page, totalPages) {
  const total = Math.max(1, Math.floor(Number(totalPages) || 1));
  const current = Math.min(total, Math.max(1, Math.floor(Number(page) || 1)));
  const raw = (current / total) * 100;
  return Math.round(raw * 100) / 100;
}

/**
 * Verifica se um locator é um EPUB CFI válido (`epubcfi(...)`).
 * @param {string|null|undefined} locator
 * @returns {boolean}
 */
export function isEpubCfi(locator) {
  if (typeof locator !== 'string') return false;
  const s = locator.trim();
  return s.startsWith('epubcfi(') && s.endsWith(')') && s.length > 9;
}

/**
 * Formata percentual para exibição em pt-BR (ex.: 42.6 -> "43%", 0 -> "0%").
 * @param {number|null|undefined} percent
 * @returns {string}
 */
export function formatPercent(percent) {
  const n = Number(percent);
  if (!Number.isFinite(n) || n <= 0) return '0%';
  if (n >= 99.5) return '100%';
  if (n < 1) return '1%';
  return `${Math.round(n)}%`;
}

/**
 * Formata tamanho em bytes para string legível em pt-BR.
 * @param {number} bytes
 * @returns {string}
 */
export function formatBytes(bytes) {
  const b = Math.max(0, Number(bytes) || 0);
  if (b < 1024) return `${Math.round(b)} B`;
  const kb = b / 1024;
  if (kb < 1024) return `${kb.toFixed(kb >= 100 ? 0 : 1).replace('.', ',')} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(mb >= 100 ? 0 : 1).replace('.', ',')} MB`;
  const gb = mb / 1024;
  return `${gb.toFixed(2).replace('.', ',')} GB`;
}

/**
 * Deriva estilo determinístico de capa placeholder (inicial + matiz HSL + selo de formato)
 * a partir de `book_id`, `title` e `format`.
 * @param {string} bookId
 * @param {string} title
 * @param {string} format
 * @returns {{ initial: string, hue: number, bgGradient: string, badge: string }}
 */
export function deriveCoverStyle(bookId, title, format = 'epub') {
  const idStr = String(bookId || '00000000');
  let hash = 2166136261;
  for (let i = 0; i < idStr.length; i++) {
    hash ^= idStr.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  const hue = Math.abs(hash) % 360;
  const hue2 = (hue + 38) % 360;
  const cleanTitle = String(title || '').trim();
  const firstAlphaMatch = cleanTitle.match(/[\p{L}\p{N}]/u);
  const initial = (firstAlphaMatch ? firstAlphaMatch[0] : '?').toUpperCase();
  const badge = String(format || 'epub').toUpperCase();
  const bgGradient = `linear-gradient(145deg, hsl(${hue}, 58%, 34%), hsl(${hue2}, 64%, 22%))`;
  return { initial, hue, bgGradient, badge };
}

/**
 * Normaliza texto para busca insensível a maiúsculas e acentos.
 * @param {string} text
 * @returns {string}
 */
export function normalizeSearchText(text) {
  return String(text || '')
    .normalize('NFD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLowerCase()
    .trim();
}

/**
 * Filtra livros por título e separa a lista de "Continuar lendo"
 * (ordenada decrescentemente por `position.updated_at_ms`).
 * @param {Array<object>} books
 * @param {string} query
 * @returns {{ continueReading: Array<object>, filteredBooks: Array<object> }}
 */
export function filterAndSortLibrary(books, query = '') {
  const list = Array.isArray(books) ? books : [];
  const normQuery = normalizeSearchText(query);

  const filteredBooks = list
    .filter((b) => {
      if (!normQuery) return true;
      return normalizeSearchText(b.title).includes(normQuery);
    })
    .slice()
    .sort((a, b) => naturalCompare(a.title || '', b.title || ''));

  const continueReading = filteredBooks
    .filter((b) => {
      const pos = b.position;
      return pos && Number(pos.updated_at_ms) > 0 && Number(pos.percent) > 0;
    })
    .slice()
    .sort((a, b) => Number(b.position.updated_at_ms) - Number(a.position.updated_at_ms));

  return { continueReading, filteredBooks };
}

/**
 * Normaliza um caminho de pasta do servidor (`a/b`, sem barras nas pontas; `""` = raiz).
 * @param {unknown} p
 * @returns {string}
 */
export function normalizeFolderPath(p) {
  if (typeof p !== 'string') return '';
  return p
    .split('/')
    .filter((seg) => seg !== '' && seg !== '.' && seg !== '..')
    .join('/');
}

/**
 * Monta a visao de uma pasta a partir da lista plana de livros. As pastas sao DERIVADAS dos
 * livros (campo `folder`), entao uma pasta sem nenhum EPUB/PDF/CBZ (nem nas subpastas) nunca
 * aparece.
 * @param {Array<object>} books
 * @param {string} currentPath pasta atual (`""` = raiz)
 * @returns {{ folders: Array<{name: string, path: string, count: number}>, books: Array<object> }}
 */
export function buildFolderView(books, currentPath = '') {
  const cur = normalizeFolderPath(currentPath);
  const prefix = cur === '' ? '' : `${cur}/`;
  const direct = [];
  const subfolders = new Map();
  for (const b of Array.isArray(books) ? books : []) {
    const f = normalizeFolderPath(b.folder);
    if (f === cur) {
      direct.push(b);
    } else if (f.startsWith(prefix)) {
      const name = f.slice(prefix.length).split('/')[0];
      const entry = subfolders.get(name) || { name, path: `${prefix}${name}`, count: 0 };
      entry.count += 1;
      subfolders.set(name, entry);
    }
  }
  return {
    folders: [...subfolders.values()].sort((a, b) => naturalCompare(a.name, b.name)),
    books: direct.sort((a, b) => naturalCompare(a.title || '', b.title || '')),
  };
}

/**
 * Migalhas de pao ate a pasta: `a/b` -> [{name:'a', path:'a'}, {name:'b', path:'a/b'}].
 * @param {string} path
 * @returns {Array<{name: string, path: string}>}
 */
export function folderBreadcrumbs(path) {
  const segs = normalizeFolderPath(path).split('/').filter(Boolean);
  return segs.map((name, i) => ({ name, path: segs.slice(0, i + 1).join('/') }));
}

/**
 * Se a pasta atual deixou de ter livros (ex.: apagada no servidor), sobe ate a pasta
 * ancestral mais proxima que ainda tem conteudo (no limite, a raiz).
 * @param {Array<object>} books
 * @param {string} path
 * @returns {string}
 */
export function resolveExistingFolder(books, path) {
  let cur = normalizeFolderPath(path);
  const list = Array.isArray(books) ? books : [];
  while (cur !== '') {
    const prefix = `${cur}/`;
    if (list.some((b) => {
      const f = normalizeFolderPath(b.folder);
      return f === cur || f.startsWith(prefix);
    })) return cur;
    cur = cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : '';
  }
  return '';
}

/**
 * Cria um debouncer (~1500 ms por padrão) com suporte a `flush()` imediato
 * (para salvar posição ao fechar/pausar o leitor) e `cancel()`.
 * @param {Function} fn
 * @param {number} delayMs
 */
export function createDebouncer(fn, delayMs = 1500) {
  let timerId = null;
  let lastArgs = null;

  function trigger(...args) {
    lastArgs = args;
    if (timerId !== null) {
      clearTimeout(timerId);
    }
    timerId = setTimeout(() => {
      timerId = null;
      const argsToRun = lastArgs;
      lastArgs = null;
      if (argsToRun !== null) {
        fn(...argsToRun);
      }
    }, delayMs);
  }

  function flush() {
    if (timerId !== null) {
      clearTimeout(timerId);
      timerId = null;
    }
    if (lastArgs !== null) {
      const argsToRun = lastArgs;
      lastArgs = null;
      return fn(...argsToRun);
    }
    return undefined;
  }

  function cancel() {
    if (timerId !== null) {
      clearTimeout(timerId);
      timerId = null;
    }
    lastArgs = null;
  }

  function isPending() {
    return timerId !== null && lastArgs !== null;
  }

  return { trigger, flush, cancel, isPending };
}
