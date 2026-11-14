#!/usr/bin/env node
// Gera todos os ícones do app (Tauri desktop, Windows, macOS, Android, iOS) a partir dos SVGs
// desta pasta. Requer apenas Node + Playwright com Chromium (rasteriza o SVG); ICO e ICNS são
// montados aqui mesmo, com PNG embutido.
//
//   node app/branding/generate-icons.mjs
//
// Fontes:
//   icon.svg          ícone com cantos arredondados (desktop)
//   icon-small.svg    versão simplificada (sem ondas) para 16–32 px
//   icon-square.svg   sem transparência, para iOS e tiles do Windows (o sistema aplica a máscara)
//   android-fg.svg    primeiro plano do ícone adaptativo (zona segura de 66 dp)
//   android-mono.svg  camada monocromática (ícones temáticos do Android 13+)

import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

const HERE = path.dirname(new URL(import.meta.url).pathname);
const ICONS = path.resolve(HERE, '../src-tauri/icons');
const ANDROID_RES = path.resolve(HERE, '../src-tauri/gen/android/app/src/main/res');
const BG_COLOR = '#1B2C3D'; // fundo do ícone adaptativo (meio do degradê do ícone)

const svg = (name) => fs.readFileSync(path.join(HERE, name), 'utf8');

const browser = await chromium.launch();
const page = await browser.newPage({ deviceScaleFactor: 1 });

/** Rasteriza um SVG em PNG quadrado. `clip: 'circle'` recorta em círculo; `inset` encolhe. */
async function png(source, size, { clip = null, inset = 0 } = {}) {
  const markup = source.replace('<svg ', '<svg width="100%" height="100%" ');
  const pad = Math.round(size * inset);
  const radius = clip === 'circle' ? '50%' : '0';
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(
    `<html><body style="margin:0;background:transparent">
      <div style="width:${size}px;height:${size}px;padding:${pad}px;box-sizing:border-box">
        <div style="width:100%;height:100%;border-radius:${radius};overflow:hidden">${markup}</div>
      </div></body></html>`,
  );
  return page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
}

function write(file, buf) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, buf);
  console.log('  ', path.relative(path.resolve(HERE, '..'), file));
}

/** ICO com entradas PNG (aceito pelo Windows Vista+). */
function buildIco(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = 6 + dir.length;
  entries.forEach(({ size, data }, i) => {
    const o = i * 16;
    dir.writeUInt8(size >= 256 ? 0 : size, o);
    dir.writeUInt8(size >= 256 ? 0 : size, o + 1);
    dir.writeUInt16LE(1, o + 4); // planos
    dir.writeUInt16LE(32, o + 6); // bpp
    dir.writeUInt32LE(data.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += data.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.data)]);
}

/** ICNS com entradas PNG. */
function buildIcns(entries) {
  const chunks = entries.map(({ type, data }) => {
    const h = Buffer.alloc(8);
    h.write(type, 0, 'ascii');
    h.writeUInt32BE(data.length + 8, 4);
    return Buffer.concat([h, data]);
  });
  const total = 8 + chunks.reduce((n, c) => n + c.length, 0);
  const h = Buffer.alloc(8);
  h.write('icns', 0, 'ascii');
  h.writeUInt32BE(total, 4);
  return Buffer.concat([h, ...chunks]);
}

const icon = svg('icon.svg');
const small = svg('icon-small.svg');
const square = svg('icon-square.svg');
const fg = svg('android-fg.svg');
const mono = svg('android-mono.svg');
const forSize = (s) => (s <= 40 ? small : icon);

console.log('Desktop (Tauri):');
for (const [file, s] of [
  ['32x32.png', 32],
  ['64x64.png', 64],
  ['128x128.png', 128],
  ['128x128@2x.png', 256],
  ['icon.png', 512],
]) {
  write(path.join(ICONS, file), await png(forSize(s), s));
}

console.log('Windows:');
const icoEntries = [];
for (const s of [16, 24, 32, 48, 64, 128, 256]) icoEntries.push({ size: s, data: await png(forSize(s), s) });
write(path.join(ICONS, 'icon.ico'), buildIco(icoEntries));
for (const s of [30, 44, 71, 89, 107, 142, 150, 284, 310]) {
  write(path.join(ICONS, `Square${s}x${s}Logo.png`), await png(s <= 44 ? small : square, s));
}
write(path.join(ICONS, 'StoreLogo.png'), await png(square, 50));

console.log('macOS:');
// Grade da Apple: o desenho ocupa ~80% do quadro, com respiro transparente em volta.
const mac = async (s) => png(s <= 64 ? small : icon, s, { inset: 0.1 });
write(
  path.join(ICONS, 'icon.icns'),
  buildIcns([
    { type: 'icp4', data: await mac(16) },
    { type: 'icp5', data: await mac(32) },
    { type: 'ic11', data: await mac(32) },
    { type: 'icp6', data: await mac(64) },
    { type: 'ic12', data: await mac(64) },
    { type: 'ic07', data: await mac(128) },
    { type: 'ic08', data: await mac(256) },
    { type: 'ic13', data: await mac(256) },
    { type: 'ic09', data: await mac(512) },
    { type: 'ic14', data: await mac(512) },
    { type: 'ic10', data: await mac(1024) },
  ]),
);

console.log('iOS:');
for (const [name, s] of [
  ['AppIcon-20x20@1x', 20], ['AppIcon-20x20@2x', 40], ['AppIcon-20x20@2x-1', 40], ['AppIcon-20x20@3x', 60],
  ['AppIcon-29x29@1x', 29], ['AppIcon-29x29@2x', 58], ['AppIcon-29x29@2x-1', 58], ['AppIcon-29x29@3x', 87],
  ['AppIcon-40x40@1x', 40], ['AppIcon-40x40@2x', 80], ['AppIcon-40x40@2x-1', 80], ['AppIcon-40x40@3x', 120],
  ['AppIcon-60x60@2x', 120], ['AppIcon-60x60@3x', 180],
  ['AppIcon-76x76@1x', 76], ['AppIcon-76x76@2x', 152], ['AppIcon-83.5x83.5@2x', 167],
  ['AppIcon-512@2x', 1024],
]) {
  write(path.join(ICONS, 'ios', `${name}.png`), await png(s <= 40 ? small : square, s));
}

console.log('Android:');
const densities = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 };
const adaptiveXml = `<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
  <background android:drawable="@color/ic_launcher_background"/>
  <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
  <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>
</adaptive-icon>
`;
const bgXml = `<?xml version="1.0" encoding="utf-8"?>
<resources>
  <color name="ic_launcher_background">${BG_COLOR}</color>
</resources>
`;
for (const root of [path.join(ICONS, 'android'), ANDROID_RES]) {
  if (!fs.existsSync(root) && root === ANDROID_RES) continue;
  for (const [d, k] of Object.entries(densities)) {
    const dir = path.join(root, `mipmap-${d}`);
    write(path.join(dir, 'ic_launcher.png'), await png(icon, Math.round(48 * k), { inset: 0.04 }));
    write(path.join(dir, 'ic_launcher_round.png'), await png(square, Math.round(48 * k), { clip: 'circle', inset: 0.04 }));
    write(path.join(dir, 'ic_launcher_foreground.png'), await png(fg, Math.round(108 * k)));
    write(path.join(dir, 'ic_launcher_monochrome.png'), await png(mono, Math.round(108 * k)));
  }
  write(path.join(root, 'mipmap-anydpi-v26', 'ic_launcher.xml'), adaptiveXml);
  write(path.join(root, 'mipmap-anydpi-v26', 'ic_launcher_round.xml'), adaptiveXml);
  write(path.join(root, 'values', 'ic_launcher_background.xml'), bgXml);
}

await browser.close();
console.log('Pronto.');
