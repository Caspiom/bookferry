# Bibliotecas Frontend Vendorizadas (`app/src/vendor/`)

Este diretório contém as bibliotecas de terceiros vendorizadas em JavaScript (ES Modules) utilizadas pelos leitores de EPUB e PDF do Bookferry, sem necessidade de bundler ou `npm install`.

| Diretório | Biblioteca | Versão / Revisão | Licença | Origem |
|---|---|---|---|---|
| `foliate-js/` | `foliate-js` (John Factotum) | Revisão vendorizada do spike (`78914aef4466eb960965702401634c2cb348e9b1`, 2026-05-01; copiada de `spike/app/src/vendor/foliate-js`) | MIT | `https://github.com/johnfactotum/foliate-js` |
| `foliate-js/vendor/fflate.js` | `fflate` (Arjun Barrett, embutido pelo `foliate-js`) | `0.8.x` (bundle original de `foliate-js/vendor/fflate.js`) | MIT | `https://github.com/101arrowz/fflate` |
| `foliate-js/vendor/zip.js` | `@zip.js/zip.js` (Gildas Lormeau, embutido pelo `foliate-js`) | `2.7.x` (bundle original de `foliate-js/vendor/zip.js`) | BSD-3-Clause | `https://github.com/gildas-lormeau/zip.js` |
| `pdfjs/` | `pdfjs-dist` (Mozilla PDF.js: `pdf.min.mjs` + `pdf.worker.min.mjs`) | `4.10.38` (copiada de `spike/app/src/vendor/pdfjs`) | Apache-2.0 | `https://github.com/mozilla/pdf.js` / `https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-4.10.38.tgz` |

Consulte [`../../THIRD_PARTY_LICENSES`](../../THIRD_PARTY_LICENSES) na raiz de `app/` para os textos completos das licenças.
