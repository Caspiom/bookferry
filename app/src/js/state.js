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
  const hue2 = (hue + 22) % 360;
  const cleanTitle = String(title || '').trim();
  const firstAlphaMatch = cleanTitle.match(/[\p{L}\p{N}]/u);
  const initial = (firstAlphaMatch ? firstAlphaMatch[0] : '?').toUpperCase();
  const badge = String(format || 'epub').toUpperCase();
  // Tons de tecido de encadernação: pouca saturação, escuro o bastante para o título claro.
  const bgGradient = `linear-gradient(160deg, hsl(${hue}, 30%, 38%), hsl(${hue2}, 36%, 24%))`;
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
 * Nome mostrado na UI: o nome escolhido pelo usuário (`display_title`) ou o título do arquivo.
 * @param {{ title?: string, display_title?: string|null }} book
 * @returns {string}
 */
export function bookDisplayTitle(book) {
  const custom = typeof book?.display_title === 'string' ? book.display_title.trim() : '';
  return custom || String(book?.title || '');
}

/**
 * `true` se o usuário tirou o livro de "Continuar lendo" e não leu de novo desde então.
 * Ler depois de esconder (posição mais nova que o instante de esconder) traz o livro de volta
 * sozinho; os dois instantes vêm do relógio do servidor, então não dependem do aparelho.
 * @param {{ hidden_continue_at_ms?: number|null, position?: { updated_at_ms?: number }|null }} book
 * @returns {boolean}
 */
export function isHiddenFromContinue(book) {
  const hiddenAt = Number(book?.hidden_continue_at_ms) || 0;
  if (hiddenAt <= 0) return false;
  const readAt = Number(book?.position?.updated_at_ms) || 0;
  return readAt <= hiddenAt;
}

/**
 * Transforma o nome de um arquivo/título em texto de busca de capa: tira extensão, grupos
 * entre ()/[]/{} (ex.: "(Digital) [Zone-Empire]"), troca `_` e `.` por espaço.
 * @param {string} title
 * @returns {string}
 */
export function cleanSearchQuery(title) {
  return String(title || '')
    .trim()
    .replace(/\.(epub|pdf|cbz|cbr|mobi|azw3|fb2)$/i, '')
    .replace(/[([{][^)\]}]*[)\]}]/g, ' ')
    .replace(/[_.]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export const OPEN_LIBRARY_SEARCH_LIMIT = 12;

/** URL de busca do Open Library (sem chave de API). */
export function openLibrarySearchUrl(query) {
  const params = new URLSearchParams({
    q: String(query || '').trim(),
    limit: String(OPEN_LIBRARY_SEARCH_LIMIT),
    fields: 'key,title,author_name,cover_i,first_publish_year',
  });
  return `https://openlibrary.org/search.json?${params.toString()}`;
}

/**
 * URL da imagem de capa. `size`: `S`, `M` ou `L`. `default=false` faz o servidor responder 404
 * quando não há capa (em vez de uma imagem de 1 px).
 * @param {number|string} coverId
 * @param {'S'|'M'|'L'} size
 * @returns {string}
 */
export function openLibraryCoverUrl(coverId, size = 'M') {
  const id = Number(coverId);
  const s = size === 'S' || size === 'L' ? size : 'M';
  if (!Number.isInteger(id) || id <= 0) return '';
  return `https://covers.openlibrary.org/b/id/${id}-${s}.jpg?default=false`;
}

/**
 * Lê a resposta do Open Library: só obras com capa, sem capas repetidas.
 * @param {unknown} json
 * @returns {Array<{ coverId: number, title: string, author: string, year: number|null }>}
 */
export function parseOpenLibraryResults(json) {
  const docs = Array.isArray(json?.docs) ? json.docs : [];
  const seen = new Set();
  const out = [];
  for (const d of docs) {
    const coverId = Number(d?.cover_i);
    if (!Number.isInteger(coverId) || coverId <= 0 || seen.has(coverId)) continue;
    seen.add(coverId);
    out.push({
      coverId,
      title: String(d.title || '').trim() || 'Sem título',
      author: Array.isArray(d.author_name) ? String(d.author_name[0] || '').trim() : '',
      year: Number.isInteger(d.first_publish_year) ? d.first_publish_year : null,
    });
  }
  return out;
}

/**
 * Menor retângulo que cabe em `maxW`×`maxH` mantendo a proporção; nunca amplia.
 * @param {number} w
 * @param {number} h
 * @param {number} maxW
 * @param {number} maxH
 * @returns {{ width: number, height: number }}
 */
export function fitWithin(w, h, maxW, maxH) {
  const sw = Number(w);
  const sh = Number(h);
  if (!(sw > 0) || !(sh > 0)) return { width: 0, height: 0 };
  const scale = Math.min(1, maxW / sw, maxH / sh);
  return {
    width: Math.max(1, Math.round(sw * scale)),
    height: Math.max(1, Math.round(sh * scale)),
  };
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

  // A busca acha tanto pelo nome escolhido quanto pelo nome original do arquivo.
  const filteredBooks = list
    .filter((b) => {
      if (!normQuery) return true;
      return (
        normalizeSearchText(bookDisplayTitle(b)).includes(normQuery) ||
        normalizeSearchText(b.title).includes(normQuery)
      );
    })
    .slice()
    .sort((a, b) => naturalCompare(bookDisplayTitle(a), bookDisplayTitle(b)));

  const continueReading = filteredBooks
    .filter((b) => {
      const pos = b.position;
      return (
        pos && Number(pos.updated_at_ms) > 0 && Number(pos.percent) > 0 && !isHiddenFromContinue(b)
      );
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

// ============================================================================
// Leitura confortável: preferências validadas, CSS do EPUB, zonas de toque, teclado,
// linha de progresso, páginas duplas de quadrinhos (CBZ) e zoom por toque duplo.
// Tudo puro (sem DOM) e coberto por `app/tests/logic.test.mjs`.
// ============================================================================

/** Valores permitidos para os campos enumerados das preferências (espelha o Rust). */
export const READER_ENUMS = Object.freeze({
  font_family: ['book', 'serif', 'sans', 'mono'],
  align: ['justify', 'left'],
  theme: ['auto', 'light', 'sepia', 'dark', 'black'],
  direction: ['ltr', 'rtl'],
  spread_mode: ['single', 'double', 'auto'],
  fit: ['page', 'width', 'height', 'original'],
  bg: ['theme', 'black', 'light'],
});

export const DEFAULT_READER_PREFS = Object.freeze({
  font_size: 18,
  font_family: 'book',
  line_height: 1.65,
  margin_pct: 6,
  align: 'justify',
  hyphenate: false,
  theme: 'auto',
  direction: 'ltr',
  spread_mode: 'single',
  cover_alone: true,
  fit: 'page',
  zero_margin: false,
  bg: 'theme',
});

export const DEFAULT_PROGRESS_PREFS = Object.freeze({
  chapter: true,
  book_percent: true,
  chapter_percent: true,
  pages_left: true,
  page_format: 0,
});

/** Campos de estilo do EPUB (os únicos copiados ao aplicar um preset). */
export const EPUB_STYLE_FIELDS = Object.freeze([
  'font_size',
  'font_family',
  'line_height',
  'margin_pct',
  'align',
  'hyphenate',
  'theme',
]);

export const MAX_PRESET_NAME_CHARS = 24;
export const MAX_USER_PRESETS = 12;

function clampNumber(value, min, max, fallback) {
  const n = typeof value === 'number' ? value : Number.NaN;
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

function pickEnum(value, allowed, fallback) {
  return typeof value === 'string' && allowed.includes(value) ? value : fallback;
}

/**
 * Valida/clampa preferências de leitura vindas de qualquer fonte (Rust, preset, UI).
 * Campo ausente, inválido ou fora da faixa cai no padrão ou é limitado; nunca lança.
 * @param {unknown} raw
 * @returns {typeof DEFAULT_READER_PREFS}
 */
export function sanitizeReaderPrefs(raw) {
  const src = raw && typeof raw === 'object' ? raw : {};
  const d = DEFAULT_READER_PREFS;
  const lh = clampNumber(src.line_height, 1.2, 2.2, d.line_height);
  return {
    font_size: Math.round(clampNumber(src.font_size, 12, 36, d.font_size)),
    font_family: pickEnum(src.font_family, READER_ENUMS.font_family, d.font_family),
    line_height: Math.round(lh * 20) / 20,
    margin_pct: Math.round(clampNumber(src.margin_pct, 0, 20, d.margin_pct)),
    align: pickEnum(src.align, READER_ENUMS.align, d.align),
    hyphenate: typeof src.hyphenate === 'boolean' ? src.hyphenate : d.hyphenate,
    theme: pickEnum(src.theme, READER_ENUMS.theme, d.theme),
    direction: pickEnum(src.direction, READER_ENUMS.direction, d.direction),
    spread_mode: pickEnum(src.spread_mode, READER_ENUMS.spread_mode, d.spread_mode),
    cover_alone: typeof src.cover_alone === 'boolean' ? src.cover_alone : d.cover_alone,
    fit: pickEnum(src.fit, READER_ENUMS.fit, d.fit),
    zero_margin: typeof src.zero_margin === 'boolean' ? src.zero_margin : d.zero_margin,
    bg: pickEnum(src.bg, READER_ENUMS.bg, d.bg),
  };
}

/**
 * Valida as preferências globais da linha de progresso.
 * @param {unknown} raw
 */
export function sanitizeProgressPrefs(raw) {
  const src = raw && typeof raw === 'object' ? raw : {};
  const d = DEFAULT_PROGRESS_PREFS;
  const flag = (v, def) => (typeof v === 'boolean' ? v : def);
  const fmt = Math.floor(clampNumber(src.page_format, 0, PAGED_FORMAT_COUNT - 1, 0));
  return {
    chapter: flag(src.chapter, d.chapter),
    book_percent: flag(src.book_percent, d.book_percent),
    chapter_percent: flag(src.chapter_percent, d.chapter_percent),
    pages_left: flag(src.pages_left, d.pages_left),
    page_format: fmt,
  };
}

/**
 * Valida o nome de um preset (mesmas regras do Rust): 1..24 caracteres, sem controle, `<` ou `>`.
 * @param {unknown} name
 * @returns {{ ok: true, name: string } | { ok: false, error: string }}
 */
export function validatePresetName(name) {
  const trimmed = typeof name === 'string' ? name.trim() : '';
  const len = Array.from(trimmed).length;
  if (len === 0) return { ok: false, error: 'Informe um nome para o preset.' };
  if (len > MAX_PRESET_NAME_CHARS) {
    return { ok: false, error: `O nome pode ter no máximo ${MAX_PRESET_NAME_CHARS} caracteres.` };
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f<>]/.test(trimmed)) {
    return { ok: false, error: 'O nome não pode ter caracteres de controle, < ou >.' };
  }
  return { ok: true, name: trimmed };
}

/**
 * Copia para `base` apenas os campos de estilo do EPUB de um preset (não mexe nos de CBZ).
 * @param {object} base preferências atuais do livro
 * @param {object} presetPrefs
 */
export function applyPresetToPrefs(base, presetPrefs) {
  const p = sanitizeReaderPrefs(presetPrefs);
  const out = sanitizeReaderPrefs(base);
  for (const f of EPUB_STYLE_FIELDS) out[f] = p[f];
  return out;
}

/** Presets embutidos (valores já dentro das faixas; só estilo do EPUB). */
export const BUILTIN_PRESETS = Object.freeze([
  { name: 'Padrão', prefs: sanitizeReaderPrefs({}) },
  {
    name: 'Papel/sépia',
    prefs: sanitizeReaderPrefs({
      font_size: 20,
      font_family: 'serif',
      line_height: 1.7,
      margin_pct: 8,
      theme: 'sepia',
    }),
  },
  {
    name: 'Noturno preto',
    prefs: sanitizeReaderPrefs({
      font_size: 20,
      font_family: 'sans',
      line_height: 1.6,
      margin_pct: 6,
      align: 'left',
      theme: 'black',
    }),
  },
]);

/** Cores do conteúdo do livro e do "chrome" por tema efetivo. */
export const READER_THEME_COLORS = Object.freeze({
  light: { bg: '#fbf8f3', fg: '#22252b', link: '#9a4a1a', dark: false },
  sepia: { bg: '#f4ead6', fg: '#46372a', link: '#8c4a1c', dark: false },
  dark: { bg: '#16191e', fg: '#e4ded3', link: '#e9a271', dark: true },
  black: { bg: '#000000', fg: '#c9c4ba', link: '#e0a070', dark: true },
});

/**
 * Resolve o tema efetivo (`light|sepia|dark|black`) do leitor.
 * `auto` segue o tema do app (se fixo) ou o esquema de cores do sistema.
 * @param {string} readerTheme
 * @param {string} appTheme `auto|light|dark|sepia|black` (Ajustes)
 * @param {boolean} prefersDark
 */
export function resolveEffectiveTheme(readerTheme, appTheme = 'auto', prefersDark = false) {
  if (readerTheme && readerTheme !== 'auto' && READER_THEME_COLORS[readerTheme]) return readerTheme;
  if (appTheme && appTheme !== 'auto' && READER_THEME_COLORS[appTheme]) return appTheme;
  return prefersDark ? 'dark' : 'light';
}

/** Pilhas de fontes do sistema (nada remoto; a CSP atual continua valendo). */
export const FONT_STACKS = Object.freeze({
  serif: "Georgia, 'Times New Roman', 'Noto Serif', 'DejaVu Serif', serif",
  sans: "system-ui, -apple-system, 'Segoe UI', Roboto, 'Noto Sans', 'DejaVu Sans', sans-serif",
  mono: "ui-monospace, 'SFMono-Regular', Menlo, Consolas, 'DejaVu Sans Mono', monospace",
});

/**
 * Monta o CSS injetado no documento do livro SOMENTE a partir de enums/números validados.
 * @param {object} rawPrefs
 * @param {string} effectiveTheme `light|sepia|dark|black`
 * @returns {string}
 */
export function buildEpubCss(rawPrefs, effectiveTheme) {
  const prefs = sanitizeReaderPrefs(rawPrefs);
  const colors = READER_THEME_COLORS[effectiveTheme] || READER_THEME_COLORS.light;
  const textBlocks =
    'body :is(p, li, dd, blockquote):not([align]):not([class*="center" i]):not([class*="title" i]):not([class*="titulo" i])';
  const parts = [
    `:root { color-scheme: ${colors.dark ? 'dark' : 'light'}; }`,
    `html, body { background: ${colors.bg} !important; color: ${colors.fg} !important; }`,
    `a, a:link, a:visited { color: ${colors.link} !important; }`,
    `html, body { font-size: ${prefs.font_size}px !important; line-height: ${prefs.line_height} !important; margin: 0 !important; }`,
    `body :is(p, li, dd, blockquote, div) { line-height: ${prefs.line_height} !important; }`,
    `${textBlocks} { text-align: ${prefs.align === 'left' ? 'left' : 'justify'} !important; }`,
    `html, body, ${textBlocks} { hyphens: ${prefs.hyphenate ? 'auto' : 'manual'} !important; -webkit-hyphens: ${prefs.hyphenate ? 'auto' : 'manual'} !important; }`,
    'p { margin: 0 0 0.75em 0; }',
    'img, svg, video { max-width: 100%; }',
  ];
  const stack = FONT_STACKS[prefs.font_family];
  if (stack) {
    parts.push(
      `html, body, body :not(pre):not(code):not(kbd):not(samp) { font-family: ${stack} !important; }`,
    );
  }
  return parts.join('\n');
}

/**
 * Atributos do `foliate-paginator` derivados das preferências. A margem lateral vira o atributo
 * `gap` (em % da largura): o paginator usa `gap/2` de cada lado na grade externa e `gap/2` de
 * preenchimento no documento, ou seja, margem lateral visível = `gap` e o mesmo valor entre colunas.
 * @param {object} rawPrefs
 */
export function epubLayoutAttrs(rawPrefs) {
  const prefs = sanitizeReaderPrefs(rawPrefs);
  return { gap: `${prefs.margin_pct}%` };
}

/**
 * Zona de toque/clique: ~30% esquerda, ~40% centro (alterna imersivo), ~30% direita.
 * Em RTL (quadrinhos/mangá) as laterais se invertem.
 * @param {number} x posição horizontal relativa ao elemento
 * @param {number} width largura do elemento
 * @param {'ltr'|'rtl'} [direction]
 * @returns {'prev'|'next'|'toggle'}
 */
export function resolveTapZone(x, width, direction = 'ltr') {
  const w = Number(width);
  const px = Number(x);
  if (!Number.isFinite(w) || !Number.isFinite(px) || w <= 0) return 'toggle';
  const ratio = px / w;
  let zone = 'toggle';
  if (ratio < 0.3) zone = 'prev';
  else if (ratio > 0.7) zone = 'next';
  if (direction === 'rtl' && zone !== 'toggle') zone = zone === 'prev' ? 'next' : 'prev';
  return zone;
}

/**
 * Ação de teclado do leitor (desktop). Os atalhos seguem os leitores populares (SumatraPDF, Evince,
 * Okular, calibre, MComix); a lista completa está em `docs/atalhos` do site e na ajuda (`?`).
 *
 * Teclas de seta devolvem a direção FÍSICA (`arrow-*`): com zoom elas movem a página, sem zoom
 * viram virada de página (ver `arrowFallbackAction`). As demais devolvem a ação já resolvida.
 * @param {{key: string, shiftKey?: boolean, ctrlKey?: boolean, altKey?: boolean, metaKey?: boolean}} ev
 * @returns {'next'|'prev'|'first'|'last'|'arrow-left'|'arrow-right'|'arrow-up'|'arrow-down'
 *   |'zoom-in'|'zoom-out'|'zoom-reset'|'toggle-fit'|'toggle-toc'|'goto'|'help'
 *   |'toggle-immersive'|'toggle-fullscreen'|'escape'|null}
 */
export function resolveKeyAction(ev) {
  if (!ev || typeof ev.key !== 'string') return null;
  if (ev.altKey || ev.metaKey) return null;
  if (ev.ctrlKey) {
    switch (ev.key) {
      case '=':
      case '+':
        return 'zoom-in';
      case '-':
      case '_':
        return 'zoom-out';
      case '0':
        return 'zoom-reset';
      case 'g':
      case 'G':
        return 'goto';
      case 'ArrowDown':
        return 'next';
      case 'ArrowUp':
        return 'prev';
      default:
        return null;
    }
  }
  switch (ev.key) {
    case 'ArrowRight':
      return 'arrow-right';
    case 'ArrowLeft':
      return 'arrow-left';
    case 'ArrowDown':
      return 'arrow-down';
    case 'ArrowUp':
      return 'arrow-up';
    case 'PageDown':
    case 'n':
    case 'N':
      return 'next';
    case 'PageUp':
    case 'p':
    case 'P':
    case 'Backspace':
      return 'prev';
    case ' ':
    case 'Spacebar':
      return ev.shiftKey ? 'prev' : 'next';
    case 'Home':
      return 'first';
    case 'End':
      return 'last';
    case '+':
    case '=':
      return 'zoom-in';
    case '-':
    case '_':
      return 'zoom-out';
    case '0':
      return 'zoom-reset';
    case 'w':
    case 'W':
      return 'toggle-fit';
    case 't':
    case 'T':
      return 'toggle-toc';
    case 'g':
    case 'G':
      return 'goto';
    case '?':
    case 'F1':
      return 'help';
    case 'f':
    case 'F':
      return 'toggle-immersive';
    case 'F11':
      return 'toggle-fullscreen';
    case 'Escape':
      return 'escape';
    default:
      return null;
  }
}

/**
 * O que uma seta faz quando a página NÃO pode ser movida (sem zoom): virar a página.
 * Esquerda/direita seguem o sentido de leitura (mangá/RTL inverte); cima/baixo voltam/avançam.
 * @param {'arrow-left'|'arrow-right'|'arrow-up'|'arrow-down'} action
 * @param {'ltr'|'rtl'} [direction]
 * @returns {'next'|'prev'|null}
 */
export function arrowFallbackAction(action, direction = 'ltr') {
  const rtl = direction === 'rtl';
  switch (action) {
    case 'arrow-right':
      return rtl ? 'prev' : 'next';
    case 'arrow-left':
      return rtl ? 'next' : 'prev';
    case 'arrow-down':
      return 'next';
    case 'arrow-up':
      return 'prev';
    default:
      return null;
  }
}

/** Quanto uma seta move a página ampliada (px). Shift não muda: Sumatra usa Shift para "mais rápido". */
export const ARROW_PAN_STEP_PX = 60;

/**
 * Deslocamento (px) que uma seta aplica à página ampliada: positivo = move o conteúdo para a
 * direita/baixo, como arrastar com o mouse.
 * @param {string} action
 * @returns {{dx: number, dy: number}|null}
 */
export function arrowPanDelta(action) {
  const step = ARROW_PAN_STEP_PX;
  switch (action) {
    case 'arrow-left':
      return { dx: step, dy: 0 };
    case 'arrow-right':
      return { dx: -step, dy: 0 };
    case 'arrow-up':
      return { dx: 0, dy: step };
    case 'arrow-down':
      return { dx: 0, dy: -step };
    default:
      return null;
  }
}

/**
 * Atalhos da biblioteca (tela inicial no desktop). Só valem com a biblioteca à vista, sem
 * modal aberto e sem foco em campo de texto (quem chama confere isso).
 * @param {{key: string, shiftKey?: boolean, ctrlKey?: boolean, altKey?: boolean, metaKey?: boolean}} ev
 * @returns {'search'|'refresh'|'settings'|'help'|'up'|null}
 */
export function resolveLibraryKeyAction(ev) {
  if (!ev || typeof ev.key !== 'string') return null;
  if (ev.metaKey) return null;
  if (ev.ctrlKey && !ev.altKey) {
    switch (ev.key) {
      case 'f':
      case 'F':
        return 'search';
      case 'r':
      case 'R':
        return 'refresh';
      case ',':
        return 'settings';
      default:
        return null;
    }
  }
  if (ev.altKey) return ev.key === 'ArrowLeft' && !ev.ctrlKey ? 'up' : null;
  switch (ev.key) {
    case '/':
      return 'search';
    case 'F5':
      return 'refresh';
    case '?':
    case 'F1':
      return 'help';
    case 'Backspace':
      return 'up';
    default:
      return null;
  }
}

/** Pasta pai de um caminho da biblioteca (`a/b` -> `a`; `a` -> ``; `` -> ``). */
export function parentFolderPath(path) {
  const parts = String(path ?? '').split('/').filter(Boolean);
  return parts.slice(0, -1).join('/');
}

/**
 * Converte o roteiro de uma roda do mouse/trackpad em pixels (`deltaMode` 1 = linhas, 2 = páginas).
 * @param {{deltaY?: number, deltaMode?: number}} ev
 */
export function normalizeWheelDelta(ev) {
  const dy = Number(ev?.deltaY);
  if (!Number.isFinite(dy)) return 0;
  if (ev.deltaMode === 1) return dy * 16;
  if (ev.deltaMode === 2) return dy * 400;
  return dy;
}

/**
 * Vira a roda do mouse em viradas de página sem pular várias de uma vez:
 * - acumula `deltaY` até `threshold` (uma "marcha" de mouse ~100 px vira 1 página; trackpad
 *   precisa de um deslize maior);
 * - depois de virar, ignora a roda por `cooldownMs` (a inércia do trackpad não vira 5 páginas);
 * - pausa maior que `gestureGapMs` ou mudança de sentido zera o acumulado.
 * Puro: quem chama passa o relógio, então dá para testar sem timers.
 * @param {{threshold?: number, cooldownMs?: number, gestureGapMs?: number}} [opts]
 * @returns {{ feed: (deltaPx: number, nowMs: number) => -1|0|1, reset: () => void }}
 */
export function createWheelPager({ threshold = 40, cooldownMs = 220, gestureGapMs = 180 } = {}) {
  let acc = 0;
  let lastEventAt = -Infinity;
  let lastFireAt = -Infinity;
  return {
    feed(deltaPx, nowMs) {
      const d = Number(deltaPx);
      if (!Number.isFinite(d) || d === 0) return 0;
      if (nowMs - lastEventAt > gestureGapMs || Math.sign(d) !== Math.sign(acc)) acc = 0;
      lastEventAt = nowMs;
      if (nowMs - lastFireAt < cooldownMs) return 0;
      acc += d;
      if (Math.abs(acc) < threshold) return 0;
      const dir = acc > 0 ? 1 : -1;
      acc = 0;
      lastFireAt = nowMs;
      return dir;
    },
    reset() {
      acc = 0;
      lastEventAt = -Infinity;
      lastFireAt = -Infinity;
    },
  };
}

/**
 * Interpreta o texto de "Ir para...": número de página (PDF/CBZ) ou percentual (EPUB).
 * Aceita "42", " 42 ", "42%" (o % é ignorado). Fora da faixa vira o limite mais próximo.
 * @param {string} text
 * @param {number} max último valor válido (total de páginas, ou 100)
 * @returns {number|null} inteiro em [1, max] (ou [0, max] com `min = 0`), ou null se inválido
 */
export function parseGotoTarget(text, max, min = 1) {
  const m = String(text ?? '').trim().match(/^(\d{1,7})\s*%?$/);
  if (!m) return null;
  const hi = Math.max(min, Math.floor(Number(max)) || min);
  return Math.max(min, Math.min(hi, Number(m[1])));
}

/** Formatos do texto de progresso em PDF/CBZ (cada clique cicla). */
export const PAGED_FORMAT_COUNT = 3;

export function nextPagedFormat(current) {
  const n = Math.floor(Number(current)) || 0;
  return (((n + 1) % PAGED_FORMAT_COUNT) + PAGED_FORMAT_COUNT) % PAGED_FORMAT_COUNT;
}

/**
 * Percentual curto de uma fração 0..1 (0 -> "0%", >= 99,5% -> "100%").
 * @param {number|null|undefined} fraction
 * @returns {string} string vazia se desconhecido
 */
export function formatFractionPercent(fraction) {
  const f = Number(fraction);
  if (fraction === null || fraction === undefined || !Number.isFinite(f)) return '';
  if (f <= 0) return '0%';
  if (f >= 0.995) return '100%';
  return `${Math.round(f * 100)}%`;
}

function pagesLeftText(n) {
  if (n <= 0) return 'última página';
  return n === 1 ? 'falta 1 pág.' : `faltam ${n} págs.`;
}

/**
 * Linha de progresso do rodapé.
 * - EPUB (`kind: 'epub'`): capítulo, % do livro, % do capítulo e páginas restantes no capítulo,
 *   conforme `items`; itens desconhecidos (null/NaN) são omitidos, nunca inventados.
 * - PDF/CBZ (`kind: 'paged'`): `pageFormat` 0 "Pág. X / Y", 1 "N%", 2 "faltam K págs.".
 * @param {object} info
 * @param {object} [items] `DEFAULT_PROGRESS_PREFS`
 * @param {number} [pageFormat]
 * @returns {string}
 */
export function formatProgressLine(info, items = DEFAULT_PROGRESS_PREFS, pageFormat = 0) {
  const it = { ...DEFAULT_PROGRESS_PREFS, ...(items || {}) };
  if (!info || typeof info !== 'object') return '';

  if (info.kind === 'paged') {
    const page = Math.max(1, Math.floor(Number(info.page) || 1));
    const total = Math.floor(Number(info.totalPages));
    const hasTotal = Number.isFinite(total) && total >= 1;
    const end = Math.floor(Number(info.pageEnd));
    const pageText =
      Number.isFinite(end) && end > page ? `${page}–${end}` : String(page);
    const last = Number.isFinite(end) && end > page ? end : page;
    const label = Number.isFinite(end) && end > page ? 'Págs.' : 'Pág.';
    const fmt = Math.floor(Number(pageFormat)) || 0;
    if (!hasTotal) return `${label} ${pageText}`;
    if (fmt === 1) return formatFractionPercent(Math.min(1, last / total));
    if (fmt === 2) return pagesLeftText(Math.max(0, total - last));
    return `${label} ${pageText} / ${total}`;
  }

  const parts = [];
  const title = typeof info.chapterTitle === 'string' ? info.chapterTitle.trim() : '';
  if (it.chapter && title) parts.push(title);
  const book = formatFractionPercent(info.bookFraction);
  if (it.book_percent && book) parts.push(book);
  const singleChapter = Number(info.totalChapters) === 1;
  const chap = formatFractionPercent(info.chapterFraction);
  if (it.chapter_percent && chap && !singleChapter) parts.push(`cap. ${chap}`);
  const left = Number(info.pagesLeftInChapter);
  if (it.pages_left && info.pagesLeftInChapter !== null && Number.isFinite(left) && left >= 0) {
    parts.push(`${pagesLeftText(Math.floor(left))} no capítulo`);
  }
  if (parts.length === 0 && book) return book;
  return parts.join(' · ');
}

/**
 * Pareia páginas de quadrinhos em "spreads" (visão de uma ou duas páginas).
 * Páginas largas (largura > altura, scans de página dupla) ficam sozinhas; a capa fica sozinha
 * se `coverAlone`. Dimensão desconhecida (`null`) conta como retrato.
 * `display` lista os índices (0-based) da esquerda para a direita; em RTL a página de menor
 * índice fica à direita.
 * @param {Array<{w:number,h:number}|null|undefined>} pageDims
 * @param {{coverAlone?: boolean, direction?: 'ltr'|'rtl'}} [opts]
 * @returns {Array<{pages: number[], display: number[], wide: boolean}>}
 */
export function computeSpreads(pageDims, { coverAlone = true, direction = 'ltr' } = {}) {
  const dims = Array.isArray(pageDims) ? pageDims : [];
  const isWide = (d) => Boolean(d) && Number(d.w) > Number(d.h);
  const spreads = [];
  let i = 0;
  while (i < dims.length) {
    const alone = isWide(dims[i]) || (i === 0 && coverAlone);
    const canPair = !alone && i + 1 < dims.length && !isWide(dims[i + 1]);
    if (canPair) {
      const pages = [i, i + 1];
      spreads.push({
        pages,
        display: direction === 'rtl' ? [i + 1, i] : pages.slice(),
        wide: false,
      });
      i += 2;
    } else {
      spreads.push({ pages: [i], display: [i], wide: isWide(dims[i]) });
      i += 1;
    }
  }
  return spreads;
}

/**
 * Índice do spread que contém a página (0-based); fora do intervalo faz clamp.
 * @param {Array<{pages:number[]}>} spreads
 * @param {number} pageIndex
 */
export function spreadIndexForPage(spreads, pageIndex) {
  if (!Array.isArray(spreads) || spreads.length === 0) return 0;
  const p = Math.floor(Number(pageIndex));
  if (!Number.isFinite(p) || p <= 0) return 0;
  const idx = spreads.findIndex((s) => s.pages.includes(p));
  return idx >= 0 ? idx : spreads.length - 1;
}

/**
 * Decide se o modo é de uma ou duas páginas. `auto`: duas só em janela paisagem e se duas páginas
 * lado a lado (na proporção típica) cabem sem encolher demais.
 * @param {'single'|'double'|'auto'} mode
 * @param {number} viewportW
 * @param {number} viewportH
 * @param {Array<{w:number,h:number}|null|undefined>} [pageDims]
 * @returns {'single'|'double'}
 */
export function resolveSpreadMode(mode, viewportW, viewportH, pageDims = []) {
  if (mode === 'double') return 'double';
  if (mode !== 'auto') return 'single';
  const w = Number(viewportW);
  const h = Number(viewportH);
  if (!(w > 0) || !(h > 0)) return 'single';
  const viewAspect = w / h;
  const aspects = (Array.isArray(pageDims) ? pageDims : [])
    .filter((d) => d && d.w > 0 && d.h > 0 && d.w <= d.h)
    .map((d) => d.w / d.h);
  const pageAspect = aspects.length ? aspects.reduce((a, b) => a + b, 0) / aspects.length : 0.7;
  return viewAspect >= 1.1 && pageAspect * 2 <= viewAspect * 1.25 ? 'double' : 'single';
}

/** Escala alvo do toque duplo: volta a 1x se já está ampliado, senão amplia (~2,5x). */
export function nextDoubleTapScale(currentScale, zoomedScale = 2.5) {
  return Number(currentScale) > 1.05 ? 1 : zoomedScale;
}

/**
 * Deslocamento (translate, com `transform-origin` no centro) para ampliar até `toScale` trazendo o
 * ponto tocado ao centro da área visível, sem mostrar além das bordas do conteúdo.
 * @param {{px:number,py:number,stageW:number,stageH:number,contentW:number,contentH:number,
 *   fromScale?:number,toScale:number,tx?:number,ty?:number}} a px/py relativos ao canto da área
 * @returns {{tx:number,ty:number}}
 */
export function zoomTranslateForPoint(a) {
  const fromScale = a.fromScale > 0 ? a.fromScale : 1;
  const toScale = a.toScale > 0 ? a.toScale : 1;
  if (toScale <= 1.001) return { tx: 0, ty: 0 };
  const ux = (a.px - a.stageW / 2 - (a.tx || 0)) / fromScale;
  const uy = (a.py - a.stageH / 2 - (a.ty || 0)) / fromScale;
  const limX = Math.max(0, (a.contentW * toScale - a.stageW) / 2);
  const limY = Math.max(0, (a.contentH * toScale - a.stageH) / 2);
  const clamp = (v, lim) => Math.max(-lim, Math.min(lim, v));
  return { tx: clamp(-ux * toScale, limX), ty: clamp(-uy * toScale, limY) };
}

/**
 * Spreads de uma página por vez (modo "uma página").
 * @param {number} count
 */
export function computeSingleSpreads(count) {
  const n = Math.max(0, Math.floor(Number(count) || 0));
  return Array.from({ length: n }, (_, i) => ({ pages: [i], display: [i], wide: false }));
}

/**
 * Tamanho do spread na tela. As páginas do spread (ordem de exibição, esquerda -> direita) são
 * normalizadas para a mesma altura; `fit` escolhe a escala:
 * `page` (cabe inteira, ocupando o máximo), `width`, `height` ou `original` (1 px de imagem = 1 px).
 * @param {Array<{w:number,h:number}>} pageSizes tamanhos naturais, em ordem de exibição
 * @param {{w:number,h:number}} stage área disponível (já sem margens)
 * @param {'page'|'width'|'height'|'original'} fit
 * @returns {{contentW:number, contentH:number, scale:number, pages: Array<{w:number,h:number}>}}
 */
export function computeSpreadLayout(pageSizes, stage, fit = 'page') {
  const sizes = (Array.isArray(pageSizes) ? pageSizes : []).filter(
    (d) => d && d.w > 0 && d.h > 0,
  );
  const sw = Math.max(1, Number(stage?.w) || 1);
  const sh = Math.max(1, Number(stage?.h) || 1);
  if (sizes.length === 0) return { contentW: sw, contentH: sh, scale: 1, pages: [] };
  const h0 = Math.max(...sizes.map((d) => d.h));
  const norm = sizes.map((d) => ({ w: (d.w * h0) / d.h, h: h0 }));
  const naturalW = norm.reduce((a, d) => a + d.w, 0);
  let scale;
  if (fit === 'width') scale = sw / naturalW;
  else if (fit === 'height') scale = sh / h0;
  else if (fit === 'original') scale = 1;
  else scale = Math.min(sw / naturalW, sh / h0);
  return {
    contentW: naturalW * scale,
    contentH: h0 * scale,
    scale,
    pages: norm.map((d) => ({ w: d.w * scale, h: d.h * scale })),
  };
}

/**
 * Limites de deslocamento (pan) para um conteúdo (já com zoom aplicado) numa área.
 * @returns {{limX:number, limY:number}}
 */
export function panLimits(contentW, contentH, stageW, stageH) {
  return {
    limX: Math.max(0, (contentW - stageW) / 2),
    limY: Math.max(0, (contentH - stageH) / 2),
  };
}

/**
 * Pan inicial de um spread maior que a área: mostra o topo e, na horizontal, o início da leitura
 * (esquerda em LTR, direita em RTL).
 * @returns {{tx:number, ty:number}}
 */
export function initialPan(contentW, contentH, stageW, stageH, direction = 'ltr') {
  const { limX, limY } = panLimits(contentW, contentH, stageW, stageH);
  return { tx: direction === 'rtl' ? -limX : limX, ty: limY };
}

/**
 * Confere se o texto lido de um QR Code e o payload de pareamento do Bookferry (JSON do servidor
 * desktop: `v`=1, `ips`, `port`, `cert_sha256`, `spki_sha256`, `secret`). Serve para a camera
 * ignorar QR de outras coisas (links, Wi-Fi) em vez de tentar parear com eles. A validacao de
 * verdade (formato dos hashes, expiracao do segredo) continua no Rust.
 * @param {unknown} text
 * @returns {string|null} o JSON original (sem espacos nas pontas) ou null se nao for do Bookferry
 */
export function parsePairingPayload(text) {
  if (typeof text !== 'string') return null;
  const trimmed = text.trim();
  if (!trimmed.startsWith('{')) return null;
  let obj;
  try {
    obj = JSON.parse(trimmed);
  } catch {
    return null;
  }
  if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return null;
  const filled = (v) => typeof v === 'string' && v.trim() !== '';
  const ok =
    obj.v === 1 &&
    Array.isArray(obj.ips) &&
    obj.ips.length > 0 &&
    obj.ips.every(filled) &&
    Number.isInteger(obj.port) &&
    obj.port >= 1 &&
    obj.port <= 65535 &&
    filled(obj.cert_sha256) &&
    filled(obj.spki_sha256) &&
    filled(obj.secret);
  return ok ? trimmed : null;
}

/**
 * Mensagem para o usuario quando a camera nao abre (erro de `getUserMedia`).
 * @param {{ name?: string }|null|undefined} err
 * @returns {string}
 */
export function cameraErrorMessage(err) {
  switch (err?.name) {
    case 'NotAllowedError':
    case 'SecurityError':
      return 'Sem permissão para usar a câmera. Libere em Configurações do Android > Apps > Bookferry > Permissões, ou cole o código.';
    case 'NotFoundError':
    case 'OverconstrainedError':
      return 'Nenhuma câmera encontrada neste aparelho. Cole o código de pareamento.';
    case 'NotReadableError':
    case 'AbortError':
      return 'A câmera está em uso por outro app. Feche-o e tente de novo.';
    default:
      return 'Não foi possível abrir a câmera. Cole o código de pareamento.';
  }
}

/**
 * Aviso para quando o servidor embutido não está na porta fixa (a única liberada no firewall
 * do PC), ou `null` se está tudo certo.
 * @param {{running?: boolean, port?: number, preferred_port?: number, port_is_fallback?: boolean}|null} server
 * @returns {string|null}
 */
export function portFallbackWarning(server) {
  if (!server || !server.running || !server.port_is_fallback) return null;
  return (
    `A porta ${server.preferred_port} está ocupada, então o servidor subiu na ${server.port}. ` +
    `O firewall do computador pode bloquear essa porta e o celular não conseguiria parear. ` +
    `Feche o programa que usa a ${server.preferred_port} e tente de novo.`
  );
}
