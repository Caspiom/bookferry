// Testes unitários automatizados da lógica pura do frontend (executados com `node --test`).

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  buildFolderView,
  computePagePercent,
  createDebouncer,
  deriveCoverStyle,
  filterAndSortLibrary,
  folderBreadcrumbs,
  formatBytes,
  formatPageLocator,
  formatPercent,
  isEpubCfi,
  naturalCompare,
  normalizeFolderPath,
  parsePageLocator,
  resolveExistingFolder,
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
