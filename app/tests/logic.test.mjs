// Testes unitários automatizados da lógica pura do frontend (executados com `node --test`).

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  computePagePercent,
  createDebouncer,
  deriveCoverStyle,
  filterAndSortLibrary,
  formatBytes,
  formatPageLocator,
  formatPercent,
  isEpubCfi,
  naturalCompare,
  parsePageLocator,
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
