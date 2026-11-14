#!/usr/bin/env node
// Gera o kit completo da marca Bookferry: todas as variações do logo em SVG, PDF (vetorial),
// PNG (vários tamanhos, fundo transparente), JPEG (com fundo), WebP, ícones (PNG/ICO), favicons
// para web, imagens para redes sociais/lojas e um guia de marca de uma página.
//
//   node app/branding/export-kit.mjs [pasta-de-saída]     (padrão: app/branding/kit)
//
// Requer Node + Playwright com Chromium. As fontes do desenho são os SVGs desta pasta
// (o desenho do símbolo está aqui embaixo, em `markArt`, igual a `mark.svg`).

import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

const HERE = path.dirname(new URL(import.meta.url).pathname);
const OUT = path.resolve(process.argv[2] || path.join(HERE, 'kit'));
const FONT_WOFF2 = path.resolve(HERE, '../src/fonts/literata-latin.woff2');

// ------------------------------------------------------------------ Cores

const C = {
  ink: '#1B2A3A',
  ink2: '#2E4258',
  inkDeep: '#132130',
  inkTop: '#22374B',
  paper: '#F4ECDD',
  paper2: '#E2D5BE',
  paperBg: '#F6F2EA',
  ember: '#E8874A',
  sea: '#5FA7A3',
  night: '#12161C',
};

/** Paletas por variação: [páginas, página da direita, brasa, mar, texto, fundo do JPEG]. */
const VARIANTS = {
  'cor': { page: C.ink, page2: C.ink2, ember: C.ember, sea: C.sea, text: C.ink, bg: C.paperBg, label: 'Colorido (fundo claro)' },
  'cor-fundo-escuro': { page: C.paper, page2: C.paper2, ember: C.ember, sea: C.sea, text: C.paper, bg: C.ink, label: 'Colorido (fundo escuro)' },
  'tinta': { page: C.ink, page2: C.ink, ember: C.ink, sea: C.ink, text: C.ink, bg: '#FFFFFF', label: 'Uma cor — tinta' },
  'preto': { page: '#000000', page2: '#000000', ember: '#000000', sea: '#000000', text: '#000000', bg: '#FFFFFF', label: 'Monocromático preto' },
  'branco': { page: '#FFFFFF', page2: '#FFFFFF', ember: '#FFFFFF', sea: '#FFFFFF', text: '#FFFFFF', bg: '#000000', label: 'Monocromático branco' },
};

// ------------------------------------------------------------------ Desenho

/** Símbolo (livro-barco) nas coordenadas originais 1024×1024. */
function markArt(v, { waves = true } = {}) {
  return [
    `<rect x="500" y="226" width="24" height="236" rx="12" fill="${v.page}"/>`,
    `<path d="M524 234 C584 246 640 280 708 298 C640 318 584 352 524 366 Z" fill="${v.ember}"/>`,
    `<path d="M512 458 C432 412 330 402 236 426 L236 628 C330 606 432 614 512 660 Z" fill="${v.page}"/>`,
    `<path d="M512 458 C592 412 694 402 788 426 L788 628 C694 606 592 614 512 660 Z" fill="${v.page2}"/>`,
    `<path d="M196 642 C320 614 430 626 512 678 C594 626 704 614 828 642 L770 744 C690 728 590 738 512 774 C434 738 334 728 254 744 Z" fill="${v.ember}"/>`,
    waves
      ? `<path d="M196 840 q52 -34 104 0 t104 0 t104 0 t104 0 t104 0 t104 0" fill="none" stroke="${v.sea}" stroke-width="32" stroke-linecap="round"/>`
      : '',
  ].join('');
}

// Caixa do símbolo (com respiro) nas coordenadas do desenho, já subindo 20 unidades.
const MARK_BOX = { x: 170, y: 170, w: 684, h: 690 };
const mark = (v, x, y, h) => {
  const k = h / MARK_BOX.h;
  return `<g transform="translate(${x} ${y}) scale(${k.toFixed(5)}) translate(${-MARK_BOX.x} ${-MARK_BOX.y})">${markArt(v)}</g>`;
};

const wordSrc = fs.readFileSync(path.join(HERE, 'wordmark.svg'), 'utf8');
const WORD_D = wordSrc.match(/ d="([^"]+)"/)[1];
const WORD = { x: 15.9, y: -234.6, w: 1498.4, h: 303.6 }; // caixa do texto (baseline em y=0)
const word = (v, x, baseline, scale = 1) =>
  `<path transform="translate(${x} ${baseline}) scale(${scale})" fill="${v.text}" d="${WORD_D}"/>`;

const svgDoc = (w, h, body, title) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}">\n` +
  `  <title>${title}</title>\n  ${body}\n</svg>\n`;

const LOGOS = {
  // Só o símbolo
  simbolo: (v) => svgDoc(684, 690, mark(v, 0, 0, 690), 'Bookferry — símbolo'),
  // Símbolo + nome lado a lado
  horizontal: (v) => {
    const h = 440;
    const mh = 420;
    const tx = (MARK_BOX.w * mh) / MARK_BOX.h + 40 - WORD.x;
    const w = Math.ceil(tx + WORD.x + WORD.w + 16);
    return svgDoc(w, h, mark(v, 0, 10, mh) + word(v, tx, 318), 'Bookferry — logo horizontal');
  },
  // Símbolo em cima, nome embaixo
  empilhado: (v) => {
    const pad = 40;
    const w = Math.ceil(WORD.w + pad * 2);
    const mh = 560;
    const mw = (MARK_BOX.w * mh) / MARK_BOX.h;
    const base = mh + 70 + -WORD.y;
    const h = Math.ceil(base + (WORD.y + WORD.h) + 30);
    return svgDoc(w, h, mark(v, (w - mw) / 2, 0, mh) + word(v, pad - WORD.x, base), 'Bookferry — logo empilhado');
  },
  // Só o nome
  nome: (v) => {
    const pad = 20;
    const w = Math.ceil(WORD.w + pad * 2);
    const h = Math.ceil(WORD.h + pad * 2);
    return svgDoc(w, h, word(v, pad - WORD.x, pad - WORD.y), 'Bookferry — nome');
  },
};

// Ícones de app (fundo próprio)
const iconBg = (light, id) =>
  light
    ? `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FBF7EF"/><stop offset="1" stop-color="#EDE3D1"/></linearGradient>`
    : `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.inkTop}"/><stop offset="1" stop-color="${C.inkDeep}"/></linearGradient>`;
function iconSvg({ shape = 'rounded', light = false, small = false, artScale = 1 }) {
  const v = light ? VARIANTS.cor : VARIANTS['cor-fundo-escuro'];
  // id único: vários ícones podem ficar na mesma página (guia de marca)
  const gid = `bf-bg-${shape}-${light ? 'claro' : 'escuro'}${small ? '-s' : ''}${artScale !== 1 ? '-m' : ''}`;
  const bg =
    shape === 'circle'
      ? `<circle cx="512" cy="512" r="512" fill="url(#${gid})"/>`
      : shape === 'square'
        ? `<rect width="1024" height="1024" fill="url(#${gid})"/>`
        : `<rect width="1024" height="1024" rx="${small ? 200 : 228}" fill="url(#${gid})"/>`;
  const scale = (small ? 1.12 : 1) * artScale;
  const shift = small ? -60 : -24;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">\n` +
    `  <title>Bookferry — ícone</title>\n  <defs>${iconBg(light, gid)}</defs>\n  ${bg}\n` +
    `  <g transform="translate(512 512) scale(${scale}) translate(-512 ${-512 + shift})">${markArt(v, { waves: !small })}</g>\n</svg>\n`
  );
}
const ICONS = {
  'icone': iconSvg({}),
  'icone-quadrado': iconSvg({ shape: 'square' }),
  'icone-circular': iconSvg({ shape: 'circle' }),
  'icone-simplificado': iconSvg({ small: true }),
  'icone-claro': iconSvg({ light: true }),
  'icone-claro-quadrado': iconSvg({ light: true, shape: 'square' }),
  'icone-maskable': iconSvg({ shape: 'square', artScale: 0.8 }), // zona segura de 80% (PWA)
};

// ------------------------------------------------------------------ Saída

const written = [];
function write(rel, data) {
  const file = path.join(OUT, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data);
  written.push(rel);
}

const browser = await chromium.launch();
const page = await browser.newPage({ deviceScaleFactor: 1 });

const dims = (svg) => {
  const m = svg.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/);
  return { w: Number(m[1]), h: Number(m[2]) };
};

/** Rasteriza: largura `w`; `bg` = cor de fundo (JPEG); `pad` = respiro relativo em volta. */
async function raster(svg, w, { type = 'png', bg = null, pad = 0, quality = 92 } = {}) {
  const d = dims(svg);
  const innerW = Math.round(w / (1 + pad * 2));
  const innerH = Math.round((innerW * d.h) / d.w);
  const W = w;
  const H = Math.round(innerH + innerW * pad * 2);
  const markup = svg.replace(/ width="[\d.]+" height="[\d.]+"/, ` width="${innerW}" height="${innerH}"`);
  await page.setViewportSize({ width: W, height: H });
  await page.setContent(
    `<html><body style="margin:0;background:${bg || 'transparent'}">` +
      `<div style="width:${W}px;height:${H}px;display:flex;align-items:center;justify-content:center">${markup}</div></body></html>`,
  );
  const shotType = type === 'webp' ? 'png' : type;
  let buf = await page.screenshot({
    type: shotType,
    omitBackground: !bg,
    clip: { x: 0, y: 0, width: W, height: H },
    ...(shotType === 'jpeg' ? { quality } : {}),
  });
  if (type === 'webp') {
    // Converte no próprio Chromium (canvas.toDataURL('image/webp')).
    const b64 = buf.toString('base64');
    const out = await page.evaluate(async (src) => {
      const img = new Image();
      img.src = `data:image/png;base64,${src}`;
      await img.decode();
      const c = document.createElement('canvas');
      c.width = img.width;
      c.height = img.height;
      c.getContext('2d').drawImage(img, 0, 0);
      return c.toDataURL('image/webp', 0.92).split(',')[1];
    }, b64);
    buf = Buffer.from(out, 'base64');
  }
  return buf;
}

/** PDF vetorial do SVG, página do tamanho do desenho (em pt, 1 unidade = 1/4 pt). */
async function pdf(svg, { bg = null } = {}) {
  const d = dims(svg);
  const wPx = Math.round(d.w / 2);
  const hPx = Math.round(d.h / 2);
  const markup = svg.replace(/ width="[\d.]+" height="[\d.]+"/, ` width="${wPx}" height="${hPx}"`);
  await page.setContent(
    `<html><head><style>@page{size:${wPx}px ${hPx}px;margin:0}html,body{margin:0}</style></head>` +
      `<body style="background:${bg || 'transparent'}">${markup.replace('<svg ', '<svg style="display:block" ')}</body></html>`,
  );
  return page.pdf({ width: `${wPx}px`, height: `${hPx}px`, printBackground: Boolean(bg), pageRanges: '1' });
}

function buildIco(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = 6 + dir.length;
  entries.forEach(({ size, data }, i) => {
    const o = i * 16;
    dir.writeUInt8(size >= 256 ? 0 : size, o);
    dir.writeUInt8(size >= 256 ? 0 : size, o + 1);
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(data.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += data.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.data)]);
}

fs.rmSync(OUT, { recursive: true, force: true });

// 1) Logos × variações
const SIZES = {
  simbolo: [256, 512, 1024, 2048],
  horizontal: [800, 1600, 3200],
  empilhado: [600, 1200, 2400],
  nome: [800, 1600, 3200],
};
for (const [name, build] of Object.entries(LOGOS)) {
  for (const [vName, v] of Object.entries(VARIANTS)) {
    const base = `${name}-${vName}`;
    const svg = build(v);
    console.log('logo', base);
    write(`svg/${base}.svg`, svg);
    write(`pdf/${base}.pdf`, await pdf(svg));
    for (const w of SIZES[name]) write(`png/${name}/${base}-${w}px.png`, await raster(svg, w));
    const mid = SIZES[name][Math.floor(SIZES[name].length / 2)];
    write(`webp/${base}-${mid}px.webp`, await raster(svg, mid, { type: 'webp' }));
    for (const w of SIZES[name].slice(-2)) {
      write(`jpeg/${name}/${base}-${w}px.jpg`, await raster(svg, w, { type: 'jpeg', bg: v.bg, pad: 0.12 }));
    }
  }
}

// 2) Ícones de app
const ICON_SIZES = [16, 32, 48, 64, 128, 180, 192, 256, 512, 1024];
for (const [name, svg] of Object.entries(ICONS)) {
  console.log('ícone', name);
  write(`svg/${name}.svg`, svg);
  write(`pdf/${name}.pdf`, await pdf(svg));
  for (const s of ICON_SIZES) {
    const src = s <= 40 && name === 'icone' ? ICONS['icone-simplificado'] : svg;
    write(`png/icones/${name}-${s}px.png`, await raster(src, s));
  }
  write(`webp/${name}-512px.webp`, await raster(svg, 512, { type: 'webp' }));
  const jpegBg = name.includes('quadrado') || name.includes('maskable') ? null : '#FFFFFF';
  write(`jpeg/icones/${name}-1024px.jpg`, await raster(svg, 1024, { type: 'jpeg', bg: jpegBg || C.ink }));
}
{
  const entries = [];
  for (const s of [16, 24, 32, 48, 64, 128, 256]) {
    entries.push({ size: s, data: await raster(s <= 40 ? ICONS['icone-simplificado'] : ICONS.icone, s) });
  }
  write('ico/bookferry.ico', buildIco(entries));
}

// 3) Favicons / web app
{
  const small = ICONS['icone-simplificado'];
  write('favicon/favicon.svg', small);
  const fav = [];
  for (const s of [16, 32, 48]) {
    const png = await raster(small, s);
    fav.push({ size: s, data: png });
    write(`favicon/favicon-${s}x${s}.png`, png);
  }
  write('favicon/favicon.ico', buildIco(fav));
  write('favicon/apple-touch-icon.png', await raster(ICONS['icone-quadrado'], 180));
  write('favicon/android-chrome-192x192.png', await raster(ICONS.icone, 192));
  write('favicon/android-chrome-512x512.png', await raster(ICONS.icone, 512));
  write('favicon/maskable-512x512.png', await raster(ICONS['icone-maskable'], 512));
  write(
    'favicon/site.webmanifest',
    JSON.stringify(
      {
        name: 'Bookferry',
        short_name: 'Bookferry',
        icons: [
          { src: '/android-chrome-192x192.png', sizes: '192x192', type: 'image/png' },
          { src: '/android-chrome-512x512.png', sizes: '512x512', type: 'image/png' },
          { src: '/maskable-512x512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
        theme_color: C.ink,
        background_color: C.paperBg,
        display: 'standalone',
      },
      null,
      2,
    ) + '\n',
  );
  write(
    'favicon/LEIA-ME.html',
    `<!-- Cole no <head> do site e coloque os arquivos desta pasta na raiz -->
<link rel="icon" href="/favicon.ico" sizes="48x48">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
<meta name="theme-color" content="${C.ink}">
`,
  );
}

// 4) Lojas e redes sociais (HTML com a Literata embutida)
const fontCss = fs.existsSync(FONT_WOFF2)
  ? `@font-face{font-family:Literata;font-weight:200 900;src:url(data:font/woff2;base64,${fs
      .readFileSync(FONT_WOFF2)
      .toString('base64')}) format('woff2')}`
  : '';
async function compose(w, h, html, type = 'png') {
  await page.setViewportSize({ width: w, height: h });
  await page.setContent(
    `<html><head><style>${fontCss}html,body{margin:0;width:${w}px;height:${h}px;overflow:hidden}` +
      `body{font-family:Literata,Georgia,serif}</style></head><body>${html}</body></html>`,
  );
  await page.evaluate(() => document.fonts.ready);
  return page.screenshot({ type, ...(type === 'jpeg' ? { quality: 92 } : {}) });
}
const darkHorizontal = LOGOS.horizontal(VARIANTS['cor-fundo-escuro']).replace(/ width="[\d.]+" height="[\d.]+"/, '');
const lightHorizontal = LOGOS.horizontal(VARIANTS.cor).replace(/ width="[\d.]+" height="[\d.]+"/, '');
const banner = (w, h, logoW, tagSize, dark = true) => `
  <div style="width:${w}px;height:${h}px;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:${Math.round(h * 0.06)}px;
    background:${dark ? `radial-gradient(${w}px ${h}px at 50% 0%, ${C.inkTop}, ${C.inkDeep})` : `radial-gradient(${w}px ${h}px at 50% 0%, #FFFDF8, #EFE7D8)`}">
    <div style="width:${logoW}px">${dark ? darkHorizontal : lightHorizontal}</div>
    <div style="font-size:${tagSize}px;color:${dark ? '#CFC6B6' : '#5E5648'};letter-spacing:-0.005em;text-align:center">
      Seus livros no computador. A leitura em qualquer aparelho.</div>
  </div>`;
for (const [name, w, h, lw, ts] of [
  ['og-image-1200x630', 1200, 630, 640, 30],
  ['github-social-preview-1280x640', 1280, 640, 680, 32],
  ['play-store-feature-graphic-1024x500', 1024, 500, 560, 26],
  ['twitter-header-1500x500', 1500, 500, 640, 30],
]) {
  console.log('social', name);
  write(`social/${name}.png`, await compose(w, h, banner(w, h, lw, ts)));
  write(`social/${name}.jpg`, await compose(w, h, banner(w, h, lw, ts), 'jpeg'));
  write(`social/${name}-claro.jpg`, await compose(w, h, banner(w, h, lw, ts, false), 'jpeg'));
}
write('social/play-store-icon-512x512.png', await raster(ICONS['icone-quadrado'], 512));
write('social/avatar-perfil-1024x1024.png', await raster(ICONS['icone-circular'], 1024));
write('social/avatar-perfil-1024x1024.jpg', await raster(ICONS['icone-quadrado'], 1024, { type: 'jpeg', bg: C.ink }));
{
  const wall = (w, h) => `<div style="width:${w}px;height:${h}px;display:flex;align-items:center;justify-content:center;
    background:radial-gradient(${w}px ${h}px at 50% 30%, ${C.inkTop}, ${C.inkDeep})">
    <div style="width:${Math.round(Math.min(w, h) * 0.32)}px">${LOGOS.simbolo(VARIANTS['cor-fundo-escuro']).replace(/ width="[\d.]+" height="[\d.]+"/, '')}</div></div>`;
  write('social/papel-de-parede-desktop-2560x1440.jpg', await compose(2560, 1440, wall(2560, 1440), 'jpeg'));
  write('social/papel-de-parede-celular-1170x2532.jpg', await compose(1170, 2532, wall(1170, 2532), 'jpeg'));
}

// 5) Guia de marca (uma página, PNG + PDF)
{
  const sw = (name, hex, fg, use) => `<div style="flex:1;border-radius:14px;overflow:hidden;border:1px solid #e4dbcc">
      <div style="height:110px;background:${hex};display:flex;align-items:flex-end;padding:12px;color:${fg};font:600 15px system-ui">${hex}</div>
      <div style="padding:12px;background:#fff"><div style="font-weight:650;font-size:17px">${name}</div>
      <div style="font:13px system-ui;color:#686052;margin-top:4px">${use}</div></div></div>`;
  const tile = (svg, bg, label) => `<div style="flex:1;display:flex;flex-direction:column;gap:8px">
      <div style="height:150px;border-radius:14px;background:${bg};border:1px solid #e4dbcc;display:flex;align-items:center;justify-content:center;padding:20px">
        ${svg.replace(/ width="[\d.]+" height="[\d.]+"/, ' style="display:block;height:110px;width:auto;max-width:100%"')}</div>
      <div style="font:13px system-ui;color:#686052">${label}</div></div>`;
  const html = `<div style="width:1600px;height:1130px;box-sizing:border-box;padding:64px 72px;background:#F6F2EA;color:#1E242D;display:flex;flex-direction:column;gap:34px">
    <div style="display:flex;justify-content:space-between;align-items:flex-end">
      <div><div style="font:700 13px system-ui;letter-spacing:.14em;color:#B9561F">GUIA DE MARCA</div>
      <div style="font-size:52px;font-weight:650;letter-spacing:-.02em">Bookferry</div></div>
      <div style="max-width:620px;font-size:19px;line-height:1.5;color:#4b4438">Um livro aberto que vira barco: as páginas são o convés, a capa é o casco, o marcador sobe como bandeira e as ondas levam a sua leitura de um aparelho para o outro.</div>
    </div>
    <div style="display:flex;gap:18px">
      ${tile(LOGOS.horizontal(VARIANTS.cor), '#FFFDF8', 'Horizontal — principal')}
      ${tile(LOGOS.horizontal(VARIANTS['cor-fundo-escuro']), C.ink, 'Horizontal — fundo escuro')}
      ${tile(LOGOS.empilhado(VARIANTS.cor), '#FFFDF8', 'Empilhado')}
    </div>
    <div style="display:flex;gap:18px">
      ${tile(ICONS.icone, '#EFE9DD', 'Ícone do app')}
      ${tile(ICONS['icone-claro'], '#EFE9DD', 'Ícone claro (alternativo)')}
      ${tile(LOGOS.simbolo(VARIANTS.preto), '#FFFFFF', 'Uma cor — preto')}
      ${tile(LOGOS.simbolo(VARIANTS.branco), '#000000', 'Uma cor — branco')}
    </div>
    <div style="display:flex;gap:18px">
      ${sw('Tinta', C.ink, '#F4ECDD', 'Fundo do ícone, texto do logo, botões principais')}
      ${sw('Papel', C.paper, '#1B2A3A', 'Páginas; texto sobre tinta')}
      ${sw('Brasa', C.ember, '#1B2A3A', 'Casco, bandeira, progresso de leitura')}
      ${sw('Mar', C.sea, '#1B2A3A', 'Ondas, estado “conectado”')}
    </div>
    <div style="display:flex;gap:48px;font:15px/1.5 system-ui;color:#4b4438">
      <div style="flex:1"><b style="font-family:Literata;font-size:24px;color:#1E242D;font-weight:650">Literata</b><br>Títulos, nome da marca e nomes de livros. Licença SIL OFL 1.1. Interface na fonte do sistema.</div>
      <div style="flex:1"><b style="color:#1E242D">Respiro</b><br>Deixe em volta do logo pelo menos a altura da bandeira. Tamanho mínimo: 24 px de altura no símbolo; abaixo de 32 px use o ícone simplificado (sem ondas).</div>
      <div style="flex:1"><b style="color:#1E242D">Evite</b><br>Distorcer, girar, trocar as cores fora destas combinações, contornar ou pôr o logo colorido sobre fotos sem contraste.</div>
    </div></div>`;
  write('guia-de-marca.png', await compose(1600, 1130, html));
  await page.setContent(
    `<html><head><style>${fontCss}@page{size:1600px 1130px;margin:0}html,body{margin:0}body{font-family:Literata,Georgia,serif}</style></head><body>${html}</body></html>`,
  );
  await page.evaluate(() => document.fonts.ready);
  write('guia-de-marca.pdf', await page.pdf({ width: '1600px', height: '1130px', printBackground: true, pageRanges: '1' }));
}

await browser.close();

write(
  'LEIA-ME.txt',
  `Kit de marca Bookferry
======================

svg/       vetores editáveis (Figma, Illustrator, Inkscape). Texto já convertido em curvas.
pdf/       vetores para gráfica/impressão (mesmos desenhos dos SVGs).
png/       fundo transparente, em vários tamanhos.
jpeg/      com fundo (claro, escuro, branco ou preto, conforme a variação) e respiro em volta.
webp/      versão leve para web, fundo transparente.
ico/       ícone do Windows (16–256 px).
favicon/   pacote para site (favicon.ico, .svg, apple-touch-icon, manifest).
social/    Open Graph, GitHub, Play Store, cabeçalho do X/Twitter, avatar, papéis de parede.
guia-de-marca.pdf / .png   resumo das cores, tipografia e usos.

Logos: simbolo, horizontal, empilhado, nome
Variações: cor (fundo claro), cor-fundo-escuro, tinta (uma cor), preto, branco
Ícones: icone, icone-quadrado, icone-circular, icone-simplificado (16–32 px),
        icone-claro, icone-claro-quadrado, icone-maskable (PWA/Android)

Cores: Tinta #1B2A3A · Papel #F4ECDD · Brasa #E8874A · Mar #5FA7A3
Fonte: Literata (SIL Open Font License 1.1)

Regerar: node app/branding/export-kit.mjs
`,
);
console.log(`\n${written.length} arquivos em ${OUT}`);
