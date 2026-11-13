#!/usr/bin/env node
// Gerador determinístico de fixtures sintéticas (EPUB de 3 capítulos, PDF de 5 páginas, CBZ de 6 PNGs)
// sem nenhuma dependência externa (usa apenas `node:fs`, `node:path`, `node:zlib`).

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

// Tabela CRC32 IEEE 802.3
const CRC_TABLE = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) {
    c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  }
  CRC_TABLE[n] = c >>> 0;
}

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

/**
 * Empacota entradas em um arquivo ZIP válido (suporta Stored=0 para `mimetype` do EPUB e Deflate=8).
 * @param {Array<{ name: string, data: Buffer, store?: boolean }>} entries
 * @returns {Buffer}
 */
function buildZipArchive(entries) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;

  for (const entry of entries) {
    const nameBuf = Buffer.from(entry.name, 'utf8');
    const rawData = Buffer.isBuffer(entry.data) ? entry.data : Buffer.from(entry.data, 'utf8');
    const crc = crc32(rawData);
    const useStore = Boolean(entry.store);
    const compData = useStore ? rawData : zlib.deflateRawSync(rawData, { level: 6 });
    const method = useStore ? 0 : 8;

    const localHeader = Buffer.alloc(30 + nameBuf.length);
    localHeader.writeUInt32LE(0x04034b50, 0); // assinatura Local File Header
    localHeader.writeUInt16LE(20, 4); // versão necessária (2.0)
    localHeader.writeUInt16LE(0x0800, 6); // bit 11 = UTF-8
    localHeader.writeUInt16LE(method, 8);
    localHeader.writeUInt16LE(0, 10);
    localHeader.writeUInt16LE(0x21, 12);
    localHeader.writeUInt32LE(crc, 14);
    localHeader.writeUInt32LE(compData.length, 18);
    localHeader.writeUInt32LE(rawData.length, 22);
    localHeader.writeUInt16LE(nameBuf.length, 26);
    localHeader.writeUInt16LE(0, 28);
    nameBuf.copy(localHeader, 30);

    localParts.push(localHeader, compData);

    const centralHeader = Buffer.alloc(46 + nameBuf.length);
    centralHeader.writeUInt32LE(0x02014b50, 0); // assinatura Central Directory
    centralHeader.writeUInt16LE(20, 4);
    centralHeader.writeUInt16LE(20, 6);
    centralHeader.writeUInt16LE(0x0800, 8);
    centralHeader.writeUInt16LE(method, 10);
    centralHeader.writeUInt16LE(0, 12);
    centralHeader.writeUInt16LE(0x21, 14);
    centralHeader.writeUInt32LE(crc, 16);
    centralHeader.writeUInt32LE(compData.length, 20);
    centralHeader.writeUInt32LE(rawData.length, 24);
    centralHeader.writeUInt16LE(nameBuf.length, 28);
    centralHeader.writeUInt16LE(0, 30);
    centralHeader.writeUInt16LE(0, 32);
    centralHeader.writeUInt16LE(0, 34);
    centralHeader.writeUInt16LE(0, 36);
    centralHeader.writeUInt32LE(0, 38);
    centralHeader.writeUInt32LE(offset, 42);
    nameBuf.copy(centralHeader, 46);

    centralParts.push(centralHeader);
    offset += localHeader.length + compData.length;
  }

  const centralBuf = Buffer.concat(centralParts);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralBuf.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20);

  return Buffer.concat([...localParts, centralBuf, eocd]);
}

/**
 * Gera um PNG válido (RGB, 8-bit) de dimensões `width x height` com cabeçalho colorido e faixas visuais.
 */
function generateColoredPng(width, height, bgRgb, accentRgb, pageNum) {
  const rowStride = 1 + width * 3;
  const rawPixels = Buffer.alloc(rowStride * height);

  for (let y = 0; y < height; y++) {
    const rowStart = y * rowStride;
    rawPixels[rowStart] = 0; // filtro None
    for (let x = 0; x < width; x++) {
      const px = rowStart + 1 + x * 3;
      const isBorder = x < 12 || x >= width - 12 || y < 12 || y >= height - 12;
      const isHeader = y >= 24 && y < 90 && x >= 24 && x < width - 24;
      // Barras horizontais indicando o número da página (1..6)
      const barIndex = Math.floor((y - 120) / 36);
      const inPageBar =
        y >= 120 &&
        (y - 120) % 36 < 20 &&
        barIndex >= 0 &&
        barIndex < pageNum &&
        x >= 40 &&
        x < width - 40;

      if (isBorder || isHeader || inPageBar) {
        rawPixels[px] = accentRgb[0];
        rawPixels[px + 1] = accentRgb[1];
        rawPixels[px + 2] = accentRgb[2];
      } else {
        rawPixels[px] = bgRgb[0];
        rawPixels[px + 1] = bgRgb[1];
        rawPixels[px + 2] = bgRgb[2];
      }
    }
  }

  function makeChunk(typeStr, dataBuf) {
    const lenBuf = Buffer.alloc(4);
    lenBuf.writeUInt32BE(dataBuf.length, 0);
    const typeBuf = Buffer.from(typeStr, 'ascii');
    const crcVal = crc32(Buffer.concat([typeBuf, dataBuf]));
    const crcBuf = Buffer.alloc(4);
    crcBuf.writeUInt32BE(crcVal, 0);
    return Buffer.concat([lenBuf, typeBuf, dataBuf, crcBuf]);
  }

  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type = RGB
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  const idatData = zlib.deflateSync(rawPixels, { level: 6 });
  return Buffer.concat([
    signature,
    makeChunk('IHDR', ihdr),
    makeChunk('IDAT', idatData),
    makeChunk('IEND', Buffer.alloc(0)),
  ]);
}

/**
 * Gera um EPUB 3 sintético com 3 capítulos e sumário navegável.
 */
function generateSyntheticEpub() {
  const containerXml = `<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>`;

  const contentOpf = `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:11111111-2222-3333-4444-555555555555</dc:identifier>
    <dc:title>Crônicas de Bookferry (EPUB Sintético)</dc:title>
    <dc:language>pt-BR</dc:language>
    <meta property="dcterms:modified">2026-11-13T12:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="cap1" href="cap1.xhtml" media-type="application/xhtml+xml"/>
    <item id="cap2" href="cap2.xhtml" media-type="application/xhtml+xml"/>
    <item id="cap3" href="cap3.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="cap1"/>
    <itemref idref="cap2"/>
    <itemref idref="cap3"/>
  </spine>
</package>`;

  const navXhtml = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="pt-BR">
<head><title>Sumário</title></head>
<body>
  <nav epub:type="toc" id="toc">
    <h1>Sumário</h1>
    <ol>
      <li><a href="cap1.xhtml">Capítulo 1 — A Partida na LAN</a></li>
      <li><a href="cap2.xhtml">Capítulo 2 — Sincronização por CFI</a></li>
      <li><a href="cap3.xhtml">Capítulo 3 — O Porto Seguro</a></li>
    </ol>
  </nav>
</body>
</html>`;

  function makeChapter(num, title) {
    let paragraphs = '';
    for (let p = 1; p <= 8; p++) {
      paragraphs += `<p id="p${num}_${p}"><strong>[Cap. ${num} · Parágrafo ${p}]</strong> O Bookferry sincroniza sua biblioteca de livros digitais (EPUB, PDF e CBZ) diretamente entre o computador e o celular pela rede local, preservando a posição exata de leitura por Canonical Fragment Identifier (CFI).</p>\n`;
    }
    return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" lang="pt-BR">
<head><title>${title}</title></head>
<body>
  <section>
    <h1>${title}</h1>
    ${paragraphs}
  </section>
</body>
</html>`;
  }

  return buildZipArchive([
    { name: 'mimetype', data: Buffer.from('application/epub+zip', 'ascii'), store: true },
    { name: 'META-INF/container.xml', data: Buffer.from(containerXml, 'utf8') },
    { name: 'OEBPS/content.opf', data: Buffer.from(contentOpf, 'utf8') },
    { name: 'OEBPS/nav.xhtml', data: Buffer.from(navXhtml, 'utf8') },
    {
      name: 'OEBPS/cap1.xhtml',
      data: Buffer.from(makeChapter(1, 'Capítulo 1 — A Partida na LAN'), 'utf8'),
    },
    {
      name: 'OEBPS/cap2.xhtml',
      data: Buffer.from(makeChapter(2, 'Capítulo 2 — Sincronização por CFI'), 'utf8'),
    },
    {
      name: 'OEBPS/cap3.xhtml',
      data: Buffer.from(makeChapter(3, 'Capítulo 3 — O Porto Seguro'), 'utf8'),
    },
  ]);
}

/**
 * Gera um PDF 1.4 válido de 5 páginas com tabela xref exata.
 */
function generateSyntheticPdf5Pages() {
  const objects = [];
  // Obj 1: Catalog
  objects.push('1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n');
  // Obj 2: Pages (5 páginas: objetos 4, 6, 8, 10, 12)
  objects.push(
    '2 0 obj\n<< /Type /Pages /Kids [4 0 R 6 0 R 8 0 R 10 0 R 12 0 R] /Count 5 >>\nendobj\n',
  );
  // Obj 3: Font Helvetica
  objects.push(
    '3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n',
  );

  for (let p = 1; p <= 5; p++) {
    const pageObjNum = 2 + p * 2;
    const contentObjNum = pageObjNum + 1;
    const streamContent = [
      'q',
      '0.93 0.95 0.99 rg',
      '36 36 523 720 re f',
      '0.15 0.38 0.92 rg',
      '36 690 523 66 re f',
      'BT',
      '/F1 24 Tf',
      '1 1 1 rg',
      '56 715 Td',
      `(Bookferry PDF - Pagina ${p} de 5) Tj`,
      'ET',
      'BT',
      '/F1 15 Tf',
      '0.1 0.12 0.18 rg',
      '56 620 Td',
      `(Leitor PDF via pdf.js 4.10.38 com HTTP Range 206 Partial Content.) Tj`,
      '0 -32 Td',
      `(Locator canonico desta pagina: page:${p} | Progresso: ${p * 20}%.) Tj`,
      '0 -32 Td',
      `(Zoom e pan em duas fases: CSS transform + re-render hi-res no canvas.) Tj`,
      'ET',
      'Q',
    ].join('\n');

    const streamBytes = Buffer.byteLength(streamContent, 'ascii');
    objects.push(
      `${pageObjNum} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${contentObjNum} 0 R >>\nendobj\n`,
    );
    objects.push(
      `${contentObjNum} 0 obj\n<< /Length ${streamBytes} >>\nstream\n${streamContent}\nendstream\nendobj\n`,
    );
  }

  let pdfStr = '%PDF-1.4\n%\xE2\xE3\xCF\xD3\n';
  const offsets = [0];
  for (const objStr of objects) {
    offsets.push(Buffer.byteLength(pdfStr, 'binary'));
    pdfStr += objStr;
  }

  const xrefOffset = Buffer.byteLength(pdfStr, 'binary');
  const totalObjs = objects.length + 1;
  pdfStr += `xref\n0 ${totalObjs}\n0000000000 65535 f \n`;
  for (let i = 1; i < totalObjs; i++) {
    pdfStr += `${String(offsets[i]).padStart(10, '0')} 00000 n \n`;
  }
  pdfStr += `trailer\n<< /Size ${totalObjs} /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;
  return Buffer.from(pdfStr, 'binary');
}

/**
 * Gera um CBZ sintético com 6 imagens PNG coloridas (testando ordenação natural page2 < page10)
 * e uma entrada `__MACOSX/._page1.png` que o protocolo Rust deve ignorar.
 */
function generateSyntheticCbz6Pages() {
  const palettes = [
    { name: 'page1.png', bg: [240, 245, 255], fg: [37, 99, 235] },
    { name: 'page2.png', bg: [240, 253, 244], fg: [22, 163, 74] },
    { name: 'page3.png', bg: [254, 249, 195], fg: [202, 138, 4] },
    { name: 'page4.png', bg: [255, 241, 242], fg: [225, 29, 72] },
    { name: 'page5.png', bg: [243, 232, 255], fg: [147, 51, 234] },
    { name: 'page10.png', bg: [236, 254, 255], fg: [8, 145, 178] },
  ];

  // Insere fora de ordem lexicográfica ("page10.png" antes de "page2.png") para exercitar a ordem natural
  const entries = [
    { name: '__MACOSX/._page1.png', data: Buffer.from('lixo-macos', 'utf8') },
    {
      name: 'page10.png',
      data: generateColoredPng(360, 480, palettes[5].bg, palettes[5].fg, 6),
    },
    {
      name: 'page1.png',
      data: generateColoredPng(360, 480, palettes[0].bg, palettes[0].fg, 1),
    },
    {
      name: 'page2.png',
      data: generateColoredPng(360, 480, palettes[1].bg, palettes[1].fg, 2),
    },
    {
      name: 'page3.png',
      data: generateColoredPng(360, 480, palettes[2].bg, palettes[2].fg, 3),
    },
    {
      name: 'page4.png',
      data: generateColoredPng(360, 480, palettes[3].bg, palettes[3].fg, 4),
    },
    {
      name: 'page5.png',
      data: generateColoredPng(360, 480, palettes[4].bg, palettes[4].fg, 5),
    },
  ];

  return buildZipArchive(entries);
}

const outDir = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.resolve(import.meta.dirname, 'out');

fs.mkdirSync(outDir, { recursive: true });

const epubPath = path.join(outDir, 'Cronicas_de_Bookferry.epub');
const pdfPath = path.join(outDir, 'Manual_Bookferry_5p.pdf');
const cbzPath = path.join(outDir, 'Quadrinho_Sintetico_6p.cbz');

fs.writeFileSync(epubPath, generateSyntheticEpub());
fs.writeFileSync(pdfPath, generateSyntheticPdf5Pages());
fs.writeFileSync(cbzPath, generateSyntheticCbz6Pages());

console.log(
  JSON.stringify(
    {
      outDir,
      files: [
        { name: path.basename(epubPath), bytes: fs.statSync(epubPath).size },
        { name: path.basename(pdfPath), bytes: fs.statSync(pdfPath).size },
        { name: path.basename(cbzPath), bytes: fs.statSync(cbzPath).size },
      ],
    },
    null,
    2,
  ),
);
