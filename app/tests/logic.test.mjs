// Testes unitários automatizados da lógica pura do frontend (executados com `node --test`).

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  BUILTIN_PRESETS,
  DEFAULT_READER_PREFS,
  applyPresetToPrefs,
  bookDisplayTitle,
  cleanSearchQuery,
  openLibraryCoverUrl,
  openLibrarySearchUrl,
  parseOpenLibraryResults,
  fitWithin,
  isHiddenFromContinue,
  buildEpubCss,
  buildFolderView,
  computeSingleSpreads,
  computeSpreadLayout,
  computeSpreads,
  initialPan,
  nextDoubleTapScale,
  panLimits,
  resolveSpreadMode,
  spreadIndexForPage,
  zoomTranslateForPoint,
  computePagePercent,
  createDebouncer,
  deriveCoverStyle,
  epubLayoutAttrs,
  filterAndSortLibrary,
  folderBreadcrumbs,
  formatBytes,
  formatFractionPercent,
  formatPageLocator,
  formatPercent,
  formatProgressLine,
  isEpubCfi,
  naturalCompare,
  nextPagedFormat,
  normalizeFolderPath,
  parsePageLocator,
  resolveEffectiveTheme,
  resolveExistingFolder,
  resolveKeyAction,
  resolveTapZone,
  sanitizeProgressPrefs,
  sanitizeReaderPrefs,
  validatePresetName,
} from '../src/js/state.js';

describe('Ordenação natural de páginas (naturalCompare)', () => {
  it('ordena páginas numéricas em ordem humana (page2 < page10)', () => {
    const input = ['page10.png', 'page2.png', 'page1.png', 'page20.png', 'page03.png'];
    const sorted = input.slice().sort(naturalCompare);
    assert.deepEqual(sorted, [
      'page1.png',
      'page2.png',
      'page03.png',
      'page10.png',
      'page20.png',
    ]);
  });

  it('ordena subpastas de capítulos e páginas insensível a maiúsculas', () => {
    const input = ['Cap10/p2.jpg', 'cap2/p10.jpg', 'cap2/p2.jpg'];
    const sorted = input.slice().sort(naturalCompare);
    assert.deepEqual(sorted, ['cap2/p2.jpg', 'cap2/p10.jpg', 'Cap10/p2.jpg']);
  });
});

describe('Mapeamento e validação de locators (EPUB CFI, PDF e CBZ)', () => {
  it('parsePageLocator extrai página 1-based e faz clamp em [1, totalPages]', () => {
    assert.equal(parsePageLocator('page:1', 10), 1);
    assert.equal(parsePageLocator('page:5', 10), 5);
    assert.equal(parsePageLocator('page:99', 10), 10);
    assert.equal(parsePageLocator('page:0', 10), 1);
    assert.equal(parsePageLocator('invalido', 10), 1);
    assert.equal(parsePageLocator(null, 10), 1);
  });

  it('formatPageLocator gera locator canônico page:<n>', () => {
    assert.equal(formatPageLocator(1), 'page:1');
    assert.equal(formatPageLocator(14.8), 'page:14');
    assert.equal(formatPageLocator(-3), 'page:1');
  });

  it('computePagePercent calcula percentual 1-based exato', () => {
    assert.equal(computePagePercent(1, 5), 20);
    assert.equal(computePagePercent(3, 5), 60);
    assert.equal(computePagePercent(5, 5), 100);
    assert.equal(computePagePercent(2, 6), 33.33);
  });

  it('isEpubCfi valida strings epubcfi(...)', () => {
    assert.equal(isEpubCfi('epubcfi(/6/4!/4/2/1:0)'), true);
    assert.equal(isEpubCfi('page:3'), false);
    assert.equal(isEpubCfi('epubcfi()'), false);
    assert.equal(isEpubCfi(''), false);
  });
});

describe('Formatação de progresso, bytes e capa gerada', () => {
  it('formatPercent formata limites 0%, 1% e 100%', () => {
    assert.equal(formatPercent(0), '0%');
    assert.equal(formatPercent(-5), '0%');
    assert.equal(formatPercent(0.4), '1%');
    assert.equal(formatPercent(42.4), '42%');
    assert.equal(formatPercent(99.8), '100%');
  });

  it('formatBytes formata B, KB, MB e GB em pt-BR', () => {
    assert.equal(formatBytes(512), '512 B');
    assert.equal(formatBytes(2048), '2,0 KB');
    assert.equal(formatBytes(1572864), '1,5 MB');
  });

  it('deriveCoverStyle gera inicial, matiz determinístico e selo do formato', () => {
    const s1 = deriveCoverStyle('0123456789abcdef0123456789abcdef', 'Dom Casmurro', 'epub');
    const s2 = deriveCoverStyle('0123456789abcdef0123456789abcdef', 'Dom Casmurro', 'epub');
    assert.equal(s1.initial, 'D');
    assert.equal(s1.badge, 'EPUB');
    assert.equal(s1.hue, s2.hue);
    assert.ok(s1.bgGradient.includes(`hsl(${s1.hue}`));
  });
});

describe('Filtro de biblioteca e seção Continuar lendo', () => {
  const sampleBooks = [
    {
      book_id: 'aaaa1111aaaa1111aaaa1111aaaa1111',
      title: 'Memórias Póstumas de Brás Cubas',
      format: 'epub',
      position: { percent: 35, updated_at_ms: 1000 },
    },
    {
      book_id: 'bbbb2222bbbb2222bbbb2222bbbb2222',
      title: 'A Arte da Guerra',
      format: 'pdf',
      position: { percent: 80, updated_at_ms: 5000 },
    },
    {
      book_id: 'cccc3333cccc3333cccc3333cccc3333',
      title: 'Quadrinho Sintético #1',
      format: 'cbz',
      position: null,
    },
  ];

  it('ordena Continuar lendo pela última leitura (updated_at_ms desc) e todos por título', () => {
    const { continueReading, filteredBooks } = filterAndSortLibrary(sampleBooks, '');
    assert.equal(filteredBooks.length, 3);
    assert.equal(filteredBooks[0].title, 'A Arte da Guerra');
    assert.equal(continueReading.length, 2);
    assert.equal(continueReading[0].book_id, 'bbbb2222bbbb2222bbbb2222bbbb2222');
    assert.equal(continueReading[1].book_id, 'aaaa1111aaaa1111aaaa1111aaaa1111');
  });

  it('filtra por título ignorando acentos e caixa', () => {
    const { filteredBooks } = filterAndSortLibrary(sampleBooks, 'memorias');
    assert.equal(filteredBooks.length, 1);
    assert.equal(filteredBooks[0].book_id, 'aaaa1111aaaa1111aaaa1111aaaa1111');
  });
});

describe('Debounce de salvamento de posição com flush imediato', () => {
  it('coalesce chamadas rápidas e executa imediatamente no flush()', async () => {
    const calls = [];
    const debouncer = createDebouncer((loc, pct) => {
      calls.push({ loc, pct });
    }, 80);

    debouncer.trigger('page:1', 20);
    debouncer.trigger('page:2', 40);
    debouncer.trigger('page:3', 60);
    assert.equal(debouncer.isPending(), true);
    assert.equal(calls.length, 0);

    // Flush ao fechar leitor deve persistir a última posição imediatamente
    debouncer.flush();
    assert.equal(debouncer.isPending(), false);
    assert.deepEqual(calls, [{ loc: 'page:3', pct: 60 }]);

    // Aguardar o tempo original não deve disparar chamada duplicada
    await new Promise((r) => setTimeout(r, 110));
    assert.equal(calls.length, 1);
  });
});

describe('Navegação por pastas', () => {
  const books = [
    { book_id: 'a', title: 'Solto', folder: '' },
    { book_id: 'b', title: 'PHB', folder: 'dnd/regras' },
    { book_id: 'c', title: 'DMG', folder: 'dnd/regras' },
    { book_id: 'd', title: 'Ficha 10', folder: 'dnd/campanha/Personagens' },
    { book_id: 'e', title: 'Ficha 2', folder: 'dnd/campanha/Personagens' },
    { book_id: 'f', title: 'Nemo', folder: 'hqs' },
    { book_id: 'g', title: 'Sem campo folder (servidor antigo)' },
  ];

  it('raiz: lista só pastas com livros dentro (contagem recursiva) e livros soltos', () => {
    const v = buildFolderView(books, '');
    assert.deepEqual(v.folders.map((f) => [f.name, f.path, f.count]), [
      ['dnd', 'dnd', 4],
      ['hqs', 'hqs', 1],
    ]);
    assert.deepEqual(v.books.map((b) => b.book_id).sort(), ['a', 'g']);
  });

  it('pasta sem nenhum livro nunca aparece (pastas são derivadas dos livros)', () => {
    // "jogos" existe no disco mas não tem EPUB/PDF/CBZ: o servidor não manda nenhum livro dela
    const v = buildFolderView(books, '');
    assert.equal(v.folders.some((f) => f.name === 'jogos'), false);
  });

  it('entra em subpasta e ordena livros em ordem natural', () => {
    const v = buildFolderView(books, 'dnd');
    assert.deepEqual(v.folders.map((f) => f.path), ['dnd/campanha', 'dnd/regras']);
    assert.equal(v.books.length, 0);
    const leaf = buildFolderView(books, 'dnd/campanha/Personagens');
    assert.deepEqual(leaf.books.map((b) => b.title), ['Ficha 2', 'Ficha 10']);
    assert.equal(leaf.folders.length, 0);
  });

  it('não confunde pastas com prefixo parecido (dnd vs dnd2)', () => {
    const v = buildFolderView([{ book_id: 'x', title: 'X', folder: 'dnd2/a' }], 'dnd');
    assert.equal(v.folders.length, 0);
    assert.equal(v.books.length, 0);
  });

  it('normalizeFolderPath descarta barras extras, ponto e ..', () => {
    assert.equal(normalizeFolderPath('/a//b/'), 'a/b');
    assert.equal(normalizeFolderPath('a/../b/./c'), 'a/b/c');
    assert.equal(normalizeFolderPath(undefined), '');
  });

  it('folderBreadcrumbs gera caminho acumulado', () => {
    assert.deepEqual(folderBreadcrumbs('a/b'), [
      { name: 'a', path: 'a' },
      { name: 'b', path: 'a/b' },
    ]);
    assert.deepEqual(folderBreadcrumbs(''), []);
  });

  it('resolveExistingFolder sobe até o ancestral que ainda tem livros', () => {
    assert.equal(resolveExistingFolder(books, 'dnd/regras'), 'dnd/regras');
    assert.equal(resolveExistingFolder(books, 'dnd/apagada/x'), 'dnd');
    assert.equal(resolveExistingFolder(books, 'nada/mais'), '');
    assert.equal(resolveExistingFolder(books, 'dnd'), 'dnd');
  });
});

describe('Preferências de leitura (sanitizeReaderPrefs)', () => {
  it('entrada vazia, nula ou lixo cai nos padrões', () => {
    assert.deepEqual(sanitizeReaderPrefs(undefined), DEFAULT_READER_PREFS);
    assert.deepEqual(sanitizeReaderPrefs(null), DEFAULT_READER_PREFS);
    assert.deepEqual(sanitizeReaderPrefs('x'), DEFAULT_READER_PREFS);
    assert.deepEqual(sanitizeReaderPrefs([1, 2]), DEFAULT_READER_PREFS);
  });

  it('limita números às faixas e ignora tipos errados', () => {
    const lo = sanitizeReaderPrefs({ font_size: 1, line_height: 0.1, margin_pct: -5 });
    assert.equal(lo.font_size, 12);
    assert.equal(lo.line_height, 1.2);
    assert.equal(lo.margin_pct, 0);
    const hi = sanitizeReaderPrefs({ font_size: 999, line_height: 9, margin_pct: 99 });
    assert.equal(hi.font_size, 36);
    assert.equal(hi.line_height, 2.2);
    assert.equal(hi.margin_pct, 20);
    const bad = sanitizeReaderPrefs({ font_size: '30', line_height: NaN, margin_pct: Infinity });
    assert.equal(bad.font_size, DEFAULT_READER_PREFS.font_size);
    assert.equal(bad.line_height, DEFAULT_READER_PREFS.line_height);
    assert.equal(bad.margin_pct, DEFAULT_READER_PREFS.margin_pct);
  });

  it('enums inválidos voltam ao padrão e CSS cru nunca passa', () => {
    const p = sanitizeReaderPrefs({
      font_family: 'Comic; } body { display:none',
      theme: 'red',
      align: 'center',
    });
    assert.equal(p.font_family, 'book');
    assert.equal(p.theme, 'auto');
    assert.equal(p.align, 'justify');
  });

  it('é idempotente', () => {
    const once = sanitizeReaderPrefs({ font_size: 22.4, line_height: 1.83, theme: 'black' });
    assert.deepEqual(sanitizeReaderPrefs(once), once);
    assert.equal(once.font_size, 22);
    assert.equal(once.line_height, 1.85);
  });
});

describe('CSS do EPUB e layout', () => {
  it('buildEpubCss usa só valores validados e as cores do tema', () => {
    const css = buildEpubCss(
      { font_size: 24, line_height: 1.9, font_family: 'serif', align: 'left', hyphenate: true },
      'black',
    );
    assert.match(css, /font-size: 24px !important/);
    assert.match(css, /line-height: 1\.9 !important/);
    assert.match(css, /background: #000000 !important/);
    assert.match(css, /color-scheme: dark/);
    assert.match(css, /text-align: left !important/);
    assert.match(css, /hyphens: auto !important/);
    assert.match(css, /Georgia/);
  });

  it('família "book" não força fonte; justificado e sem hifenização por padrão', () => {
    const css = buildEpubCss({}, 'light');
    assert.doesNotMatch(css, /font-family/);
    assert.match(css, /text-align: justify !important/);
    assert.match(css, /hyphens: manual !important/);
    assert.match(css, /color-scheme: light/);
  });

  it('entrada maliciosa não vaza para o CSS', () => {
    const css = buildEpubCss(
      { font_family: 'x; } * { display:none', font_size: '1px; } *{', theme: 'url(x)' },
      'nope',
    );
    assert.doesNotMatch(css, /display:\s*none/);
    assert.match(css, /font-size: 18px/);
  });

  it('a margem lateral vira o atributo gap em %', () => {
    assert.deepEqual(epubLayoutAttrs({ margin_pct: 12 }), { gap: '12%' });
    assert.deepEqual(epubLayoutAttrs({ margin_pct: 500 }), { gap: '20%' });
  });
});

describe('Tema efetivo', () => {
  it('tema do livro vence; auto segue o app; senão o sistema', () => {
    assert.equal(resolveEffectiveTheme('black', 'light', false), 'black');
    assert.equal(resolveEffectiveTheme('auto', 'sepia', true), 'sepia');
    assert.equal(resolveEffectiveTheme('auto', 'auto', true), 'dark');
    assert.equal(resolveEffectiveTheme('auto', 'auto', false), 'light');
    assert.equal(resolveEffectiveTheme('xyz', 'abc', false), 'light');
  });
});

describe('Presets', () => {
  it('validatePresetName: limites e caracteres proibidos', () => {
    assert.deepEqual(validatePresetName('  Noite  '), { ok: true, name: 'Noite' });
    assert.equal(validatePresetName('').ok, false);
    assert.equal(validatePresetName('   ').ok, false);
    assert.equal(validatePresetName(null).ok, false);
    assert.equal(validatePresetName('a'.repeat(24)).ok, true);
    assert.equal(validatePresetName('a'.repeat(25)).ok, false);
    assert.equal(validatePresetName('ação'.repeat(6)).ok, true);
    assert.equal(validatePresetName('<b>').ok, false);
    assert.equal(validatePresetName('a\nb').ok, false);
  });

  it('applyPresetToPrefs copia só o estilo do EPUB e preserva os campos de quadrinhos', () => {
    const base = sanitizeReaderPrefs({ direction: 'rtl', spread_mode: 'double', font_size: 30 });
    const out = applyPresetToPrefs(base, BUILTIN_PRESETS[2].prefs);
    assert.equal(out.theme, 'black');
    assert.equal(out.font_size, 20);
    assert.equal(out.direction, 'rtl');
    assert.equal(out.spread_mode, 'double');
  });

  it('presets embutidos já estão dentro das faixas', () => {
    for (const p of BUILTIN_PRESETS) assert.deepEqual(sanitizeReaderPrefs(p.prefs), p.prefs);
  });
});

describe('Itens do rodapé e formato de páginas', () => {
  it('sanitizeProgressPrefs aplica padrões e limita page_format', () => {
    assert.deepEqual(sanitizeProgressPrefs(null), {
      chapter: true,
      book_percent: true,
      chapter_percent: true,
      pages_left: true,
      page_format: 0,
    });
    assert.equal(sanitizeProgressPrefs({ page_format: 99 }).page_format, 2);
    assert.equal(sanitizeProgressPrefs({ chapter: 'sim' }).chapter, true);
    assert.equal(sanitizeProgressPrefs({ chapter: false }).chapter, false);
  });

  it('nextPagedFormat cicla 0 -> 1 -> 2 -> 0', () => {
    assert.equal(nextPagedFormat(0), 1);
    assert.equal(nextPagedFormat(1), 2);
    assert.equal(nextPagedFormat(2), 0);
    assert.equal(nextPagedFormat(undefined), 1);
  });

  it('formatFractionPercent: bordas', () => {
    assert.equal(formatFractionPercent(0), '0%');
    assert.equal(formatFractionPercent(-1), '0%');
    assert.equal(formatFractionPercent(0.4), '40%');
    assert.equal(formatFractionPercent(0.996), '100%');
    assert.equal(formatFractionPercent(1), '100%');
    assert.equal(formatFractionPercent(null), '');
    assert.equal(formatFractionPercent(NaN), '');
  });

  it('EPUB: linha completa e itens desligados', () => {
    const info = {
      kind: 'epub',
      chapterTitle: 'Capítulo 2',
      bookFraction: 0.5,
      chapterFraction: 0.25,
      pagesLeftInChapter: 3,
      totalChapters: 10,
    };
    assert.equal(
      formatProgressLine(info),
      'Capítulo 2 · 50% · cap. 25% · faltam 3 págs. no capítulo',
    );
    assert.equal(formatProgressLine(info, { chapter: false, pages_left: false }), '50% · cap. 25%');
    assert.equal(
      formatProgressLine(info, {
        chapter: false,
        book_percent: false,
        chapter_percent: false,
        pages_left: false,
      }),
      '50%',
    );
  });

  it('EPUB: bordas (0%, 100%, 1 capítulo, sem título, desconhecidos)', () => {
    assert.equal(
      formatProgressLine({ kind: 'epub', bookFraction: 0, chapterTitle: '' }),
      '0%',
    );
    assert.equal(
      formatProgressLine({ kind: 'epub', bookFraction: 1, pagesLeftInChapter: 0, chapterFraction: 1, totalChapters: 4 }),
      '100% · cap. 100% · última página no capítulo',
    );
    // Livro de 1 capítulo: "cap. X%" repetiria o % do livro.
    assert.equal(
      formatProgressLine({ kind: 'epub', chapterTitle: 'Único', bookFraction: 0.3, chapterFraction: 0.3, totalChapters: 1 }),
      'Único · 30%',
    );
    assert.equal(
      formatProgressLine({ kind: 'epub', bookFraction: 0.1, pagesLeftInChapter: 1 }),
      '10% · falta 1 pág. no capítulo',
    );
    // Itens que o leitor não sabe calcular são omitidos, não inventados.
    assert.equal(
      formatProgressLine({ kind: 'epub', bookFraction: null, chapterFraction: null, pagesLeftInChapter: null }),
      '',
    );
    assert.equal(formatProgressLine(null), '');
  });

  it('PDF/CBZ: três formatos e total desconhecido', () => {
    const info = { kind: 'paged', page: 3, totalPages: 10 };
    assert.equal(formatProgressLine(info, undefined, 0), 'Pág. 3 / 10');
    assert.equal(formatProgressLine(info, undefined, 1), '30%');
    assert.equal(formatProgressLine(info, undefined, 2), 'faltam 7 págs.');
    assert.equal(formatProgressLine({ kind: 'paged', page: 10, totalPages: 10 }, undefined, 2), 'última página');
    assert.equal(formatProgressLine({ kind: 'paged', page: 9, totalPages: 10 }, undefined, 2), 'falta 1 pág.');
    assert.equal(formatProgressLine({ kind: 'paged', page: 4 }, undefined, 1), 'Pág. 4');
    assert.equal(formatProgressLine({ kind: 'paged', page: 2, pageEnd: 3, totalPages: 8 }, undefined, 0), 'Págs. 2–3 / 8');
  });
});

describe('Zonas de toque e teclado', () => {
  it('resolveTapZone: 30% / 40% / 30% e inversão em RTL', () => {
    assert.equal(resolveTapZone(10, 1000), 'prev');
    assert.equal(resolveTapZone(299, 1000), 'prev');
    assert.equal(resolveTapZone(300, 1000), 'toggle');
    assert.equal(resolveTapZone(500, 1000), 'toggle');
    assert.equal(resolveTapZone(701, 1000), 'next');
    assert.equal(resolveTapZone(10, 1000, 'rtl'), 'next');
    assert.equal(resolveTapZone(990, 1000, 'rtl'), 'prev');
    assert.equal(resolveTapZone(500, 1000, 'rtl'), 'toggle');
  });

  it('resolveTapZone: entradas inválidas alternam a interface em vez de virar página', () => {
    assert.equal(resolveTapZone(10, 0), 'toggle');
    assert.equal(resolveTapZone(NaN, 100), 'toggle');
  });

  it('resolveKeyAction: setas, espaço, F, F11, Esc e RTL', () => {
    assert.equal(resolveKeyAction({ key: 'ArrowRight' }), 'next');
    assert.equal(resolveKeyAction({ key: 'ArrowRight' }, 'rtl'), 'prev');
    assert.equal(resolveKeyAction({ key: 'ArrowLeft' }, 'rtl'), 'next');
    assert.equal(resolveKeyAction({ key: ' ' }), 'next');
    assert.equal(resolveKeyAction({ key: ' ', shiftKey: true }), 'prev');
    assert.equal(resolveKeyAction({ key: 'PageDown' }, 'rtl'), 'next');
    assert.equal(resolveKeyAction({ key: 'f' }), 'toggle-immersive');
    assert.equal(resolveKeyAction({ key: 'F11' }), 'toggle-fullscreen');
    assert.equal(resolveKeyAction({ key: 'Escape' }), 'escape');
    assert.equal(resolveKeyAction({ key: '=', ctrlKey: true }), 'font-inc');
    assert.equal(resolveKeyAction({ key: 'a' }), null);
    assert.equal(resolveKeyAction({ key: 'ArrowRight', altKey: true }), null);
  });
});

describe('Quadrinhos: spreads (computeSpreads)', () => {
  const P = { w: 100, h: 150 };
  const W = { w: 300, h: 150 };

  it('capa sozinha e o resto em pares; sobra fica sozinha', () => {
    const sp = computeSpreads([P, P, P, P, P, P]);
    assert.deepEqual(sp.map((s) => s.pages), [[0], [1, 2], [3, 4], [5]]);
    assert.deepEqual(sp.map((s) => s.display), [[0], [1, 2], [3, 4], [5]]);
  });

  it('RTL inverte só a ordem de exibição (menor índice à direita)', () => {
    const sp = computeSpreads([P, P, P, P], { direction: 'rtl' });
    assert.deepEqual(sp.map((s) => s.pages), [[0], [1, 2], [3]]);
    assert.deepEqual(sp.map((s) => s.display), [[0], [2, 1], [3]]);
  });

  it('sem "capa sozinha" pareia desde a primeira página', () => {
    const sp = computeSpreads([P, P, P, P, P, P], { coverAlone: false });
    assert.deepEqual(sp.map((s) => s.pages), [[0, 1], [2, 3], [4, 5]]);
  });

  it('página larga fica sozinha e muda o pareamento seguinte', () => {
    const sp = computeSpreads([P, P, P, W, P, P]);
    assert.deepEqual(sp.map((s) => s.pages), [[0], [1, 2], [3], [4, 5]]);
    assert.equal(sp[2].wide, true);
    // Página seguida de uma larga não pareia com ela.
    const sp2 = computeSpreads([P, P, W, P]);
    assert.deepEqual(sp2.map((s) => s.pages), [[0], [1], [2], [3]]);
  });

  it('dimensão desconhecida conta como retrato; entradas vazias não quebram', () => {
    assert.deepEqual(computeSpreads([null, undefined, null]).map((s) => s.pages), [[0], [1, 2]]);
    assert.deepEqual(computeSpreads([]), []);
    assert.deepEqual(computeSpreads(null), []);
  });

  it('toda página aparece exatamente uma vez, em ordem', () => {
    const dims = [P, W, P, P, P, W, W, P, P];
    for (const direction of ['ltr', 'rtl']) {
      for (const coverAlone of [true, false]) {
        const flat = computeSpreads(dims, { coverAlone, direction }).flatMap((s) => s.pages);
        assert.deepEqual(flat, dims.map((_, i) => i));
      }
    }
  });

  it('spreadIndexForPage encontra o spread e faz clamp', () => {
    const sp = computeSpreads([P, P, P, P, P, P]);
    assert.equal(spreadIndexForPage(sp, 0), 0);
    assert.equal(spreadIndexForPage(sp, 2), 1);
    assert.equal(spreadIndexForPage(sp, 3), 2);
    assert.equal(spreadIndexForPage(sp, 99), 3);
    assert.equal(spreadIndexForPage(sp, -4), 0);
    assert.equal(spreadIndexForPage([], 5), 0);
  });

  it('computeSingleSpreads: uma página por spread', () => {
    assert.deepEqual(computeSingleSpreads(3).map((s) => s.pages), [[0], [1], [2]]);
    assert.deepEqual(computeSingleSpreads(0), []);
    assert.deepEqual(computeSingleSpreads(-2), []);
  });
});

describe('Quadrinhos: modo, layout, pan e zoom', () => {
  it('resolveSpreadMode: auto só em janela larga', () => {
    assert.equal(resolveSpreadMode('double', 100, 1000), 'double');
    assert.equal(resolveSpreadMode('single', 1600, 900), 'single');
    assert.equal(resolveSpreadMode('xyz', 1600, 900), 'single');
    assert.equal(resolveSpreadMode('auto', 1600, 900), 'double');
    assert.equal(resolveSpreadMode('auto', 900, 1600), 'single');
    assert.equal(resolveSpreadMode('auto', 0, 0), 'single');
    // Janela quase quadrada não vira duas páginas, mesmo com páginas bem estreitas.
    assert.equal(resolveSpreadMode('auto', 1000, 1000, [{ w: 40, h: 100 }]), 'single');
    // Páginas largas na amostra são ignoradas no cálculo da proporção típica.
    assert.equal(resolveSpreadMode('auto', 1600, 900, [{ w: 300, h: 100 }]), 'double');
  });

  it('computeSpreadLayout: encaixes página/largura/altura/original', () => {
    const one = [{ w: 100, h: 200 }];
    const stage = { w: 400, h: 400 };
    const page = computeSpreadLayout(one, stage, 'page');
    assert.deepEqual([page.contentW, page.contentH, page.scale], [200, 400, 2]);
    const width = computeSpreadLayout(one, stage, 'width');
    assert.deepEqual([width.contentW, width.contentH], [400, 800]);
    const height = computeSpreadLayout(one, stage, 'height');
    assert.deepEqual([height.contentW, height.contentH], [200, 400]);
    const orig = computeSpreadLayout(one, stage, 'original');
    assert.deepEqual([orig.contentW, orig.contentH], [100, 200]);
  });

  it('computeSpreadLayout: duas páginas dividem a largura e alturas diferentes se normalizam', () => {
    const two = computeSpreadLayout([{ w: 100, h: 200 }, { w: 100, h: 200 }], { w: 400, h: 400 }, 'page');
    assert.deepEqual([two.contentW, two.contentH], [400, 400]);
    assert.deepEqual(two.pages, [{ w: 200, h: 400 }, { w: 200, h: 400 }]);
    const mixed = computeSpreadLayout([{ w: 100, h: 200 }, { w: 100, h: 100 }], { w: 600, h: 400 }, 'page');
    // a segunda página é normalizada para a altura da maior (200): 200x200
    assert.equal(mixed.pages[0].h, mixed.pages[1].h);
    assert.equal(mixed.pages[1].w, mixed.pages[0].w * 2);
    const empty = computeSpreadLayout([], { w: 300, h: 200 });
    assert.deepEqual([empty.contentW, empty.contentH, empty.pages.length], [300, 200, 0]);
  });

  it('panLimits e initialPan: zero quando cabe; LTR começa à esquerda, RTL à direita', () => {
    assert.deepEqual(panLimits(1000, 800, 500, 500), { limX: 250, limY: 150 });
    assert.deepEqual(panLimits(300, 300, 500, 500), { limX: 0, limY: 0 });
    assert.deepEqual(initialPan(1000, 800, 500, 500, 'ltr'), { tx: 250, ty: 150 });
    assert.deepEqual(initialPan(1000, 800, 500, 500, 'rtl'), { tx: -250, ty: 150 });
    assert.deepEqual(initialPan(300, 300, 500, 500, 'ltr'), { tx: 0, ty: 0 });
  });

  it('nextDoubleTapScale alterna ampliar/voltar', () => {
    assert.equal(nextDoubleTapScale(1), 2.5);
    assert.equal(nextDoubleTapScale(1.05), 2.5);
    assert.equal(nextDoubleTapScale(1.06), 1);
    assert.equal(nextDoubleTapScale(2.5), 1);
    assert.equal(nextDoubleTapScale(1, 3), 3);
  });

  it('zoomTranslateForPoint centraliza o ponto tocado, respeitando as bordas', () => {
    const base = { stageW: 1000, stageH: 800, contentW: 1000, contentH: 800, fromScale: 1, toScale: 2.5 };
    const center = zoomTranslateForPoint({ ...base, px: 500, py: 400 });
    assert.equal(center.tx + 0, 0);
    assert.equal(center.ty + 0, 0);
    // toque à direita: conteúdo desloca para a esquerda, mas nunca além do limite (750)
    assert.equal(zoomTranslateForPoint({ ...base, px: 1000, py: 400 }).tx, -750);
    assert.equal(zoomTranslateForPoint({ ...base, px: 600, py: 400 }).tx, -250);
    assert.equal(zoomTranslateForPoint({ ...base, px: 0, py: 0 }).tx, 750);
    assert.equal(zoomTranslateForPoint({ ...base, px: 0, py: 0 }).ty, 600);
    assert.deepEqual(zoomTranslateForPoint({ ...base, px: 900, py: 100, toScale: 1 }), { tx: 0, ty: 0 });
  });
});

describe('nome, capa e Continuar lendo por livro', () => {
  const pos = (updated, percent = 10) => ({ updated_at_ms: updated, percent });

  it('bookDisplayTitle usa o nome escolhido, ignora vazio/espaços e cai no título', () => {
    assert.equal(bookDisplayTitle({ title: 'arq', display_title: '  Meu nome ' }), 'Meu nome');
    assert.equal(bookDisplayTitle({ title: 'arq', display_title: '   ' }), 'arq');
    assert.equal(bookDisplayTitle({ title: 'arq', display_title: null }), 'arq');
    assert.equal(bookDisplayTitle({ title: 'arq' }), 'arq');
    assert.equal(bookDisplayTitle(null), '');
  });

  it('isHiddenFromContinue: escondido só até a próxima leitura (relógio do servidor)', () => {
    assert.equal(isHiddenFromContinue({ position: pos(100) }), false);
    assert.equal(isHiddenFromContinue({ hidden_continue_at_ms: 0, position: pos(100) }), false);
    assert.equal(isHiddenFromContinue({ hidden_continue_at_ms: 500, position: pos(100) }), true);
    // leitura exatamente no instante de esconder ainda conta como "antes"
    assert.equal(isHiddenFromContinue({ hidden_continue_at_ms: 500, position: pos(500) }), true);
    assert.equal(isHiddenFromContinue({ hidden_continue_at_ms: 500, position: pos(501) }), false);
    assert.equal(isHiddenFromContinue({ hidden_continue_at_ms: 500 }), true);
  });

  it('filterAndSortLibrary: some do Continuar lendo mas continua na biblioteca; volta ao ler', () => {
    const books = [
      { book_id: 'a', title: 'Alfa', hidden_continue_at_ms: 900, position: pos(800) },
      { book_id: 'b', title: 'Beta', position: pos(700) },
      { book_id: 'c', title: 'Gama', hidden_continue_at_ms: 900, position: pos(1000) },
    ];
    const { continueReading, filteredBooks } = filterAndSortLibrary(books, '');
    assert.deepEqual(continueReading.map((b) => b.book_id), ['c', 'b']);
    assert.equal(filteredBooks.length, 3, 'o livro escondido continua na biblioteca');
  });

  it('filterAndSortLibrary: busca e ordem usam o nome escolhido, e ainda acham o original', () => {
    const books = [
      { book_id: 'a', title: 'zzz-scan-001', display_title: 'Alan Moore - Watchmen' },
      { book_id: 'b', title: 'Batman' },
    ];
    const byCustom = filterAndSortLibrary(books, 'watchmen').filteredBooks;
    assert.deepEqual(byCustom.map((b) => b.book_id), ['a']);
    const byOriginal = filterAndSortLibrary(books, 'ZZZ-scan').filteredBooks;
    assert.deepEqual(byOriginal.map((b) => b.book_id), ['a']);
    assert.deepEqual(filterAndSortLibrary(books, '').filteredBooks.map((b) => b.book_id), ['a', 'b']);
  });

  it('fitWithin reduz mantendo proporção, nunca amplia e trata entradas inválidas', () => {
    assert.deepEqual(fitWithin(1000, 2000, 360, 540), { width: 270, height: 540 });
    assert.deepEqual(fitWithin(2000, 1000, 360, 540), { width: 360, height: 180 });
    assert.deepEqual(fitWithin(100, 150, 360, 540), { width: 100, height: 150 });
    assert.deepEqual(fitWithin(0, 100, 360, 540), { width: 0, height: 0 });
    assert.deepEqual(fitWithin(NaN, 100, 360, 540), { width: 0, height: 0 });
    assert.deepEqual(fitWithin(1, 100000, 360, 540), { width: 1, height: 540 });
  });
});

describe('busca de capa na internet (Open Library)', () => {
  it('cleanSearchQuery tira extensão, grupos e separadores de nome de arquivo', () => {
    assert.equal(cleanSearchQuery('Swamp_Thing.v01 (2016) (Digital) [Zone-Empire].cbz'), 'Swamp Thing v01');
    assert.equal(cleanSearchQuery('  Dom Casmurro.epub '), 'Dom Casmurro');
    assert.equal(cleanSearchQuery('Livro (sem fechar'), 'Livro (sem fechar');
    assert.equal(cleanSearchQuery('[Grupo] Obra'), 'Obra');
    assert.equal(cleanSearchQuery('(tudo entre parênteses)'), '');
    assert.equal(cleanSearchQuery(null), '');
  });

  it('openLibrarySearchUrl codifica a consulta e limita campos', () => {
    const url = new URL(openLibrarySearchUrl('a&b=c ção'));
    assert.equal(url.origin, 'https://openlibrary.org');
    assert.equal(url.pathname, '/search.json');
    assert.equal(url.searchParams.get('q'), 'a&b=c ção');
    assert.equal(url.searchParams.get('limit'), '12');
    assert.match(url.searchParams.get('fields'), /cover_i/);
  });

  it('openLibraryCoverUrl só aceita id inteiro positivo e tamanho válido', () => {
    assert.equal(openLibraryCoverUrl(710358, 'L'), 'https://covers.openlibrary.org/b/id/710358-L.jpg?default=false');
    assert.equal(openLibraryCoverUrl('710358', 'x'), 'https://covers.openlibrary.org/b/id/710358-M.jpg?default=false');
    for (const bad of [0, -1, 1.5, 'abc', '../../x', null, undefined, NaN]) {
      assert.equal(openLibraryCoverUrl(bad), '', String(bad));
    }
  });

  it('parseOpenLibraryResults ignora obras sem capa, repetidas e respostas inválidas', () => {
    const json = {
      docs: [
        { title: 'Swamp Thing', author_name: ['Alan Moore', 'Len Wein'], cover_i: 710358, first_publish_year: 1987 },
        { title: 'Sem capa' },
        { title: 'Repetida', cover_i: 710358 },
        { cover_i: 5, author_name: [] },
        { title: 'Id ruim', cover_i: '../x' },
      ],
    };
    assert.deepEqual(parseOpenLibraryResults(json), [
      { coverId: 710358, title: 'Swamp Thing', author: 'Alan Moore', year: 1987 },
      { coverId: 5, title: 'Sem título', author: '', year: null },
    ]);
    assert.deepEqual(parseOpenLibraryResults(null), []);
    assert.deepEqual(parseOpenLibraryResults({ docs: 'x' }), []);
  });
});
