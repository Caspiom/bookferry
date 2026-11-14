// Capas dos livros: gera a capa embutida (página 1 do CBZ/PDF, capa do EPUB), redimensiona
// imagens escolhidas pelo usuário e carrega as capas já sincronizadas para os cards.
// O servidor guarda uma capa por livro; aqui só se produz/consome o JPEG já pequeno.

import * as pdfjsLib from '../vendor/pdfjs/pdf.min.mjs';
import { makeBook } from '../vendor/foliate-js/view.js';
import { api, getBookFileUrl, getCbzPageUrl } from './api.js';
import { fitWithin } from './state.js';

export const COVER_MAX_W = 360;
export const COVER_MAX_H = 540;
const COVER_JPEG_QUALITY = 0.85;
// Livros cuja capa não deu para extrair (EPUB sem capa, PDF protegido...) não são tentados
// de novo a cada abertura do app: espera 7 dias.
const FAILED_KEY = 'bookferry.coverFailed';
const FAILED_TTL_MS = 7 * 24 * 60 * 60 * 1000;

if (!pdfjsLib.GlobalWorkerOptions.workerSrc) {
  pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
    '../vendor/pdfjs/pdf.worker.min.mjs',
    import.meta.url,
  ).toString();
}

/**
 * Redimensiona uma imagem (Blob de JPEG/PNG/WebP) para caber em 360×540 e devolve JPEG.
 * Fundo branco para PNG com transparência.
 * @param {Blob} blob
 * @returns {Promise<Uint8Array>}
 */
export async function resizeImageToJpeg(blob) {
  const bitmap = await createImageBitmap(blob);
  try {
    const { width, height } = fitWithin(bitmap.width, bitmap.height, COVER_MAX_W, COVER_MAX_H);
    if (!width || !height) throw new Error('imagem sem dimensões válidas');
    return await canvasToJpegBytes(width, height, (ctx) => ctx.drawImage(bitmap, 0, 0, width, height));
  } finally {
    bitmap.close?.();
  }
}

async function canvasToJpegBytes(width, height, draw) {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = '#ffffff';
  ctx.fillRect(0, 0, width, height);
  draw(ctx);
  const out = await new Promise((resolve) => canvas.toBlob(resolve, 'image/jpeg', COVER_JPEG_QUALITY));
  if (!out) throw new Error('falha ao codificar a capa');
  return new Uint8Array(await out.arrayBuffer());
}

async function pdfFirstPageJpeg(bookId) {
  const task = pdfjsLib.getDocument({
    url: getBookFileUrl(bookId),
    rangeChunkSize: 65536,
    disableAutoFetch: true,
  });
  const doc = await task.promise;
  try {
    const page = await doc.getPage(1);
    const base = page.getViewport({ scale: 1 });
    const { width } = fitWithin(base.width, base.height, COVER_MAX_W, COVER_MAX_H);
    const viewport = page.getViewport({ scale: width / base.width });
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(viewport.width));
    canvas.height = Math.max(1, Math.round(viewport.height));
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    await page.render({ canvasContext: ctx, viewport }).promise;
    const out = await new Promise((r) => canvas.toBlob(r, 'image/jpeg', COVER_JPEG_QUALITY));
    if (!out) throw new Error('falha ao codificar a capa do PDF');
    return new Uint8Array(await out.arrayBuffer());
  } finally {
    await doc.destroy();
  }
}

/**
 * Extrai a capa do próprio arquivo (já baixado neste aparelho) como JPEG pequeno.
 * @param {{ book_id: string, format: string }} book
 * @returns {Promise<Uint8Array|null>} `null` se o arquivo não tem capa utilizável.
 */
export async function extractEmbeddedCover(book) {
  const fmt = String(book.format).toLowerCase();
  if (fmt === 'pdf') return pdfFirstPageJpeg(book.book_id);
  if (fmt === 'cbz') {
    const resp = await fetch(getCbzPageUrl(book.book_id, 0));
    if (!resp.ok) return null;
    return resizeImageToJpeg(await resp.blob());
  }
  if (fmt === 'epub') {
    const resp = await fetch(getBookFileUrl(book.book_id));
    if (!resp.ok) return null;
    const file = new File([await resp.blob()], `${book.book_id}.epub`, {
      type: 'application/epub+zip',
    });
    const epub = await makeBook(file);
    const cover = await epub.getCover?.();
    return cover ? resizeImageToJpeg(cover) : null;
  }
  return null;
}

function readFailed() {
  try {
    return JSON.parse(localStorage.getItem(FAILED_KEY) || '{}');
  } catch (_) {
    return {};
  }
}

function markFailed(bookId) {
  try {
    const all = readFailed();
    all[bookId] = Date.now();
    localStorage.setItem(FAILED_KEY, JSON.stringify(all));
  } catch (_) {}
}

function recentlyFailed(bookId) {
  const at = Number(readFailed()[bookId]) || 0;
  return at > 0 && Date.now() - at < FAILED_TTL_MS;
}

const generating = new Set();

/**
 * Gera e envia a capa embutida de um livro JÁ baixado, se ele ainda não tem capa.
 * Nunca sobrescreve uma capa existente (o servidor também recusa sobre capa do usuário).
 * @param {{ book_id: string, format: string, cover_version?: number }} book
 * @returns {Promise<boolean>} `true` se uma capa nova foi gravada
 */
export async function generateCoverIfMissing(book) {
  if (!book || Number(book.cover_version) > 0) return false;
  if (generating.has(book.book_id) || recentlyFailed(book.book_id)) return false;
  generating.add(book.book_id);
  try {
    const bytes = await extractEmbeddedCover(book);
    if (!bytes) {
      markFailed(book.book_id);
      return false;
    }
    const meta = await api.setBookCover(book.book_id, 'embedded', bytes);
    return Boolean(meta?.applied);
  } catch (_) {
    markFailed(book.book_id);
    return false;
  } finally {
    generating.delete(book.book_id);
  }
}

let backgroundRunning = false;

/**
 * Passada em segundo plano (só no desktop): para cada livro sem capa, baixa a cópia local se
 * preciso, gera a capa e remove a cópia que só existia para isso. Um livro por vez.
 * @param {() => Array<object>} getBooks lista atual (lida a cada passo, pois muda por eventos)
 */
export async function generateMissingCovers(getBooks) {
  if (backgroundRunning) return;
  backgroundRunning = true;
  try {
    const missing = await api.listBooksWithoutCover();
    for (const id of missing) {
      const book = getBooks().find((b) => b.book_id === id);
      if (!book || Number(book.cover_version) > 0 || recentlyFailed(id)) continue;
      const wasDownloaded = Boolean(book.downloaded);
      try {
        if (!wasDownloaded) await api.downloadBook(id);
        await generateCoverIfMissing(book);
      } catch (_) {
        markFailed(id);
      } finally {
        if (!wasDownloaded) {
          try {
            await api.removeDownload(id);
          } catch (_) {}
        }
      }
    }
  } catch (_) {
    // servidor fora do ar: tenta de novo na próxima atualização da biblioteca
  } finally {
    backgroundRunning = false;
  }
}

// --- Carregamento das capas nos cards ---------------------------------------------------

const coverUrls = new Map(); // `${bookId}:${versao}` -> url | null
const pendingCovers = new Map(); // mesma chave -> Promise
const MAX_PARALLEL = 4;
let active = 0;
const waiting = [];

function runLimited(task) {
  return new Promise((resolve) => {
    const start = () => {
      active += 1;
      task()
        .catch(() => null)
        .then((v) => {
          active -= 1;
          waiting.shift()?.();
          resolve(v);
        });
    };
    if (active < MAX_PARALLEL) start();
    else waiting.push(start);
  });
}

/**
 * URL da capa se já estiver resolvida (síncrono, evita piscar ao re-renderizar a lista).
 * @returns {string|null|undefined} `undefined` = ainda não resolvida
 */
export function cachedCoverUrl(book) {
  return coverUrls.get(`${book.book_id}:${book.cover_version}`);
}

/**
 * Resolve (baixando se preciso) a URL local da capa de um livro.
 * @returns {Promise<string|null>}
 */
export function resolveCoverUrl(book) {
  const version = Number(book.cover_version) || 0;
  if (version <= 0) return Promise.resolve(null);
  const key = `${book.book_id}:${version}`;
  if (coverUrls.has(key)) return Promise.resolve(coverUrls.get(key));
  if (!pendingCovers.has(key)) {
    pendingCovers.set(
      key,
      runLimited(async () => {
        const url = await api.ensureCover(book.book_id);
        // Só guarda sucesso: se estava offline, uma próxima renderização tenta de novo.
        if (url) coverUrls.set(key, url);
        return url || null;
      }).finally(() => pendingCovers.delete(key)),
    );
  }
  return pendingCovers.get(key);
}

/**
 * Coloca a capa (se houver) dentro do `coverEl` do card, trocando o placeholder.
 * @param {HTMLElement} coverEl
 * @param {object} book
 */
export function attachCover(coverEl, book) {
  if (Number(book.cover_version) <= 0) return;
  const show = (url) => {
    if (!url) return;
    const img = document.createElement('img');
    img.className = 'book-cover-img';
    img.alt = '';
    img.decoding = 'async';
    img.addEventListener('load', () => coverEl.classList.add('has-cover-image'));
    img.addEventListener('error', () => img.remove());
    img.src = url;
    coverEl.prepend(img);
  };
  const cached = cachedCoverUrl(book);
  if (cached) show(cached);
  else resolveCoverUrl(book).then(show);
}
