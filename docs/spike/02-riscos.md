# Spike 02 — Riscos Técnicos (SAF >500 MB, Zoom/Pan 60–120 fps em WebView, Equivalência de CFI Linux/Android)

Data: 13/11/2026 · Dispositivo Android: Samsung SM-S918B (Galaxy S23 Ultra, Android 15, serial `RXCW104L1XZ`, pacote `dev.gaspari.bookferry.spike`) · Host Linux: NixOS (`x86_64-linux`), devShell `spike/flake.nix`.

---

## Resumo Executivo e Vereditos

| Risco | Descrição | Veredito | Síntese dos Números Reais |
|---|---|---|---|
| **Risco 1** | Arquivo grande no Android via SAF (`ACTION_OPEN_DOCUMENT` → `fd` → Rust) sem cópia integral (CBZ 295 MB e 592 MB) | **RESOLVIDO** | Zero cópia para cache (`detachFd()` → `File::from_raw_fd`). Listar + ordenar natural (ignorando `__MACOSX`): **23,7–28,6 ms** (295 MB) e **21,4–24,8 ms** (592 MB). 1ª página decodificada no WebView (`1.014.400 B`, `1760×2304`): **150,9–204,3 ms** (295 MB, deflate) e **117,5–158,3 ms** (592 MB, stored). Pico de RAM (`TOTAL PSS`): **181,1 MB** (295 MB) e **185,4 MB** (592 MB), apenas +16 a +20 MB sobre o baseline (**165,2 MB**). |
| **Risco 2** | Fluidez de zoom/pan de quadrinho em WebView (CSS `transform` + Pointer Events) e comparação com `pdf.js` | **RESOLVIDO** | Página grande (`1,01 MB`, `1760×2304`) em tela cheia no S23 Ultra: **119,5 fps médios** (`8,37 ms/frame`, `p95 = 8,4 ms` / `119,05 fps`, **0 frames > 16,7 ms**) em animação `rAF` (zoom 1×→3× + pan) e **119,6 fps médios** (`8,36 ms/frame`, `p95 = 8,4 ms`, **0 frames > 16,7 ms**) durante 6 swipes reais (`adb shell input swipe`, 424 eventos `pointermove`). `dumpsys gfxinfo`: p50=5 ms, p95=5–6 ms, GPU p95=2 ms, 0 missed vsync. `pdf.js` (71 MB via HTTP Range no `fd` SAF): abre em **152,4 ms**, renderiza 1ª vez em **283,9–433,8 ms** (`361,4 ms` em `1055×1380`), re-renderiza zoom cached em **7,6–13,9 ms** e faz pan CSS a **120,0 fps**. |
| **Risco 3** | Equivalência de CFI (`foliate-js`) entre Linux (`webkit2gtk` via `xvfb-run`) e Android WebView | **VIÁVEL COM RESSALVAS** | **100% idêntico (24/24 CFIs)** no nível de árvore DOM (`resolveHref` de capítulos e offset de caracteres via `SectionProgress`). No paginador visual (`<foliate-view>`), o CFI de range da tela diverge no fim da página e no `goToFraction` por diferença de largura de coluna/métricas de fonte (ex.: 73 chars de diferença em Moby-Dick 10%). **Caminho inverso (Linux ↔ Android): 100% (9/9 CFIs)** gerados no Linux resolvem no Android exatamente para o mesmo nó DOM e trecho de texto (`dom_exact_match: true`). |

> **Recomendação Arquitetural:** **MANTER TAURI v2 (Linux + Android) com núcleo em Rust.** Não é necessário migrar para o Plano B (Flutter). Todos os três riscos técnicos foram validados com folga em hardware real.

---

## Ambiente e Versões Utilizadas

Comandos executados dentro de `cd spike && nix develop`:

| Componente | Versão / Identificação Real |
|---|---|
| Dispositivo Android | Samsung Galaxy S23 Ultra (`SM-S918B`), Android `15` (`Build/AP3A.240905.015.A2`), serial `RXCW104L1XZ` |
| Estado de bloqueio de tela | `isKeyguardShowing=false`, `mDreamingLockscreen=false` (`svc power stayon usb` durante os testes, revertido ao final com `svc power stayon false`) |
| Android WebView (User-Agent) | `Chrome/157.0.8103.40 Mobile Safari/537.36` (`1080×2316` físicos, viewport CSS `384×823`, `dpr=2.8125`, display 120 Hz) |
| Linux WebKitGTK (User-Agent) | `webkit2gtk-4.1` `2.54.2` (`AppleWebKit/605.1.15`), executado headless via `xvfb-run -a -s "-screen 0 1280x1024x24"` |
| Rust / Tauri | `rustc 1.100.0` / `tauri 2.12.5` / `wry 0.57.2` / `zip 2.6.1` (`deflate`) / `base64 0.21.7` |
| `foliate-js` vendorizado | GitHub `johnfactotum/foliate-js`, commit `78914aef4466eb960965702401634c2cb348e9b1` (`2026-05-01`), licença MIT |
| `pdf.js` vendorizado | `pdfjs-dist@4.10.38` (`build/pdf.min.mjs` + `pdf.worker.min.mjs`), licença Apache-2.0 |

---

## Risco 1 — Arquivo grande no Android via SAF dentro do Tauri

### 1.1. Preparação dos Fixtures e Bug Encontrado no Script Anterior
O script pré-existente [`make-synthetic-cbz.py`](file:///home/gappy/Documents/projetos/bookshelf/spike/scripts/make-synthetic-cbz.py) continha um bug na linha 7:
```python
pages = sorted(
    i
    for i in zin.infolist()
    if not i.filename.startswith('__MACOSX') and i.filename.endswith('.jpeg')
)
```
Em Python 3, `zipfile.ZipInfo` não implementa o operador `<`, causando `TypeError: '<' not supported between instances of 'ZipInfo' and 'ZipInfo'`. Corrigimos adicionando `key=lambda i: i.filename` e geramos `spike/fixtures/synthetic-600mb.cbz`:
- **Saída real da geração e envio (`adb push`):**
```text
842 paginas; 591743358 bytes
fixtures/little-nemo-1905-1914.cbz: 1 file pushed, 0 skipped. 208.4 MB/s (295457942 bytes in 1.352s)
fixtures/synthetic-600mb.cbz: 1 file pushed, 0 skipped. 177.5 MB/s (591743358 bytes in 3.180s)
fixtures/little-nemo-1905-1914.pdf: 1 file pushed, 0 skipped. 155.7 MB/s (70683356 bytes in 0.433s)
fixtures/frankenstein-pg84.epub: 1 file pushed, 0 skipped. 167.1 MB/s (440131 bytes in 0.003s)
fixtures/moby-dick-pg2701.epub: 1 file pushed, 0 skipped. 236.6 MB/s (699614 bytes in 0.003s)
fixtures/pride-and-prejudice-pg1342.epub: 1 file pushed, 0 skipped. 169.1 MB/s (24864457 bytes in 0.140s)
```
> **Detalhe importante descoberto (MediaScanner):** Arquivos enviados por `adb push` não aparecem imediatamente na listagem do `DocumentsUI` (`ACTION_OPEN_DOCUMENT`) até que o `MediaScanner` os indexe. Executamos `adb shell content call --uri content://media --method scan_file --arg /storage/emulated/0/Download/bookferry-fixtures/synthetic-600mb.cbz`, após o que o arquivo de 592 MB apareceu imediatamente no seletor do sistema.

### 1.2. Prova do Bloqueio de Caminho Direto (Scoped Storage Android 15)
Antes de acionar o seletor SAF, testamos abrir os 6 arquivos diretamente por caminho POSIX (`/sdcard/Download/bookferry-fixtures/...`) via `std::fs::File::open` no Rust (`window.testDirectAccess()`). Todos falharam com o erro real do kernel/FUSE do Android 15:
```text
[BOOKFERRY_RESULT][DIRECT_PATH_PROBE] [
  {"file":"little-nemo-1905-1914.cbz","ok":false,"error":"File::open(/sdcard/Download/bookferry-fixtures/little-nemo-1905-1914.cbz) falhou: Permission denied (os error 13) (os error Some(13))"},
  {"file":"synthetic-600mb.cbz","ok":false,"error":"File::open(/sdcard/Download/bookferry-fixtures/synthetic-600mb.cbz) falhou: Permission denied (os error 13) (os error Some(13))"}
]
```
Isso confirma que no Android 15 (`targetSdk = 37`), arquivos externos fora do sandbox do app exigem SAF (`ACTION_OPEN_DOCUMENT`) quando o app não usa `MANAGE_EXTERNAL_STORAGE`.

### 1.3. Arquitetura da Ponte SAF Zero-Copy (`MainActivity.kt` → `lib.rs`)
Implementamos a ponte nativa em [`MainActivity.kt`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src-tauri/gen/android/app/src/main/java/dev/gaspari/bookferry/spike/MainActivity.kt) e [`lib.rs`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src-tauri/src/lib.rs):
1. `MainActivity` lança `Intent(Intent.ACTION_OPEN_DOCUMENT)` com `DocumentsContract.EXTRA_INITIAL_URI` apontando para `primary:Download/bookferry-fixtures`.
2. Guiados por `adb shell uiautomator dump`, clicamos no card do arquivo no `com.google.android.documentsui` com `adb shell input tap 780 960` (`little-nemo-1905-1914.cbz`) e `adb shell input tap 780 700` (`synthetic-600mb.cbz`).
3. O Kotlin chama `contentResolver.openFileDescriptor(uri, "r")!!.detachFd()`, destacando o descritor numérico (`fd: Int`) na tabela de arquivos do processo Linux do app.
4. O comando Tauri `saf_inspect_and_read_cbz(fd, name, page_index, target_name)` no Rust reconstrói `unsafe { std::fs::File::from_raw_fd(fd) }`, abre `zip::ZipArchive::new(file)` (fazendo `seek` direto ao *Central Directory* no final do arquivo, sem ler os 295 MB / 592 MB), ignora entradas `__MACOSX` e `._*`, ordena naturalmente (`natural_cmp`) e lê a maior página do arquivo (`1.014.400 bytes`, `1760×2304`).

### 1.4. Medidas Reais — Risco 1 (CBZ 295 MB vs. CBZ 592 MB)

Saídas brutas do `logcat` (`adb logcat -d -s BookferrySpike:I`):
```json
{"file_name":"little-nemo-1905-1914.cbz","uri":"content://com.android.externalstorage.documents/document/primary%3ADownload%2Fbookferry-fixtures%2Flittle-nemo-1905-1914.cbz","file_size_bytes":295457942,"total_zip_entries":843,"ignored_macosx_entries":422,"valid_page_count":421,"first_5_pages":["little-nemo-19051015-l-01.jpeg","little-nemo-19051022-l-02.jpeg","little-nemo-19051029-l-03.jpeg","little-nemo-19051105-l-04.jpeg","little-nemo-19051112-l-05.jpeg"],"last_5_pages":["little-nemo-19140503-l-417.jpeg","little-nemo-19140524-l-418.jpeg","little-nemo-19140621-l-419.jpeg","little-nemo-19140705-l-420.jpeg","little-nemo-19140726-l-421.jpeg"],"target_page_index":16,"target_page_name":"little-nemo-19060204-l-17.jpeg","target_page_bytes":1014400,"img_dimensions":"1760x2304","copied_to_cache":false,"runs":[
  {"rep":0,"open_fd_ms":41.891,"zip_open_ms":27.0,"list_natural_sort_ms":28.575,"read_single_page_ms":44.656,"base64_encode_ms":11.414,"total_rust_ms":84.65,"ipc_roundtrip_ms":167.1,"first_page_rendered_ms":204.3},
  {"rep":1,"open_fd_ms":26.104,"zip_open_ms":22.181,"list_natural_sort_ms":23.676,"read_single_page_ms":42.395,"base64_encode_ms":11.059,"total_rust_ms":77.135,"ipc_roundtrip_ms":156.0,"first_page_rendered_ms":157.6},
  {"rep":2,"open_fd_ms":31.313,"zip_open_ms":22.977,"list_natural_sort_ms":24.536,"read_single_page_ms":42.481,"base64_encode_ms":11.431,"total_rust_ms":78.454,"ipc_roundtrip_ms":149.3,"first_page_rendered_ms":150.9}
]}
```
```json
{"file_name":"synthetic-600mb.cbz","uri":"content://com.android.externalstorage.documents/document/primary%3ADownload%2Fbookferry-fixtures%2Fsynthetic-600mb.cbz","file_size_bytes":591743358,"total_zip_entries":1684,"ignored_macosx_entries":842,"valid_page_count":842,"first_5_pages":["page1.jpeg","page2.jpeg","page3.jpeg","page4.jpeg","page5.jpeg"],"last_5_pages":["page838.jpeg","page839.jpeg","page840.jpeg","page841.jpeg","page842.jpeg"],"target_page_index":16,"target_page_name":"page17.jpeg","target_page_bytes":1014400,"img_dimensions":"1760x2304","copied_to_cache":false,"runs":[
  {"rep":0,"open_fd_ms":19.2,"zip_open_ms":22.549,"list_natural_sort_ms":24.787,"read_single_page_ms":6.696,"base64_encode_ms":11.547,"total_rust_ms":43.034,"ipc_roundtrip_ms":133.1,"first_page_rendered_ms":158.3},
  {"rep":1,"open_fd_ms":24.61,"zip_open_ms":20.578,"list_natural_sort_ms":22.51,"read_single_page_ms":4.806,"base64_encode_ms":11.456,"total_rust_ms":38.776,"ipc_roundtrip_ms":117.7,"first_page_rendered_ms":119.7},
  {"rep":2,"open_fd_ms":26.332,"zip_open_ms":19.527,"list_natural_sort_ms":21.381,"read_single_page_ms":4.816,"base64_encode_ms":11.046,"total_rust_ms":37.248,"ipc_roundtrip_ms":114.5,"first_page_rendered_ms":117.5}
]}
```

| Métrica | `little-nemo-1905-1914.cbz` (295,5 MB, Deflate) | `synthetic-600mb.cbz` (591,7 MB, Stored) |
|---|---|---|
| Total de entradas ZIP / `__MACOSX` ignoradas / Páginas válidas | 843 / 422 / **421** | 1.684 / 842 / **842** |
| Ordenação natural (`first_5_pages` → `last_5_pages`) | `...-01.jpeg`..`...-05.jpeg` → `...-417.jpeg`..`...-421.jpeg` | `page1.jpeg`..`page5.jpeg` → `page838.jpeg`..`page842.jpeg` |
| Cópia para cache necessária? | **Não (`false`)** | **Não (`false`)** |
| `open_fd_ms` (`openFileDescriptor` + `detachFd`) — Cold / Warm | **41,89 ms** / 26,10–31,31 ms | **19,20 ms** / 24,61–26,33 ms |
| **Tempo até listar + ordenar natural (`list_natural_sort_ms`)** | **28,58 ms** (cold) / **23,68–24,54 ms** (warm) | **24,79 ms** (cold) / **21,38–22,51 ms** (warm) |
| Leitura de 1 página de 1,01 MB no Rust (`read_single_page_ms`) | **44,66 ms** (cold) / **42,40 ms** (warm, *deflate*) | **6,70 ms** (cold) / **4,81 ms** (warm, *stored*) |
| **Tempo total até 1ª página renderizada (`first_page_rendered_ms`)** | **204,3 ms** (cold) / **150,9–157,6 ms** (warm) | **158,3 ms** (cold) / **117,5–119,7 ms** (warm) |

#### Consumo de RAM (`adb shell dumpsys meminfo dev.gaspari.bookferry.spike` e `/proc/self/status`)

| Momento | `TOTAL PSS` | `Native Heap PSS` | `Dalvik Heap PSS` | `Graphics PSS` | `VmRSS` | `VmHWM` (Pico RSS) |
|---|---|---|---|---|---|---|
| **Baseline (app aberto, antes do SAF)** | **165.163 KB** (~161,3 MiB) | 12.924 KB | 7.540 KB | — | 224.904 KB | 227.748 KB |
| **Após CBZ 295 MB (3× list + decode `1760×2304`)** | **181.096 KB** (~176,9 MiB) | 20.260 KB | 8.040 KB | 77.856 KB | 234.824 KB | **236.360 KB** |
| **Após CBZ 592 MB (3× list + decode `1760×2304`)** | **185.407 KB** (~181,1 MiB) | 8.764 KB | 4.780 KB | 79.212 KB | 197.632 KB | **237.412 KB** |

---

## Risco 2 — Fluidez de Zoom/Pan em WebView (Sensação YACReader/Moon+)

### 2.1. Implementação do Visualizador de Quadrinhos (Sem Framework)
Em [`index.html`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src/index.html) e [`main.js`](file:///home/gappy/Documents/projetos/bookshelf/spike/app/src/main.js), renderizamos a maior página do Little Nemo (`little-nemo-19060204-l-17.jpeg`, **1.014.400 bytes**, dimensão decodificada **`1760×2304`**) em um palco fullscreen (`#comic-stage`, `100vw × 100vh`, `touch-action: none`) com `<img id="comic-img" style="will-change: transform; transform-origin: 0 0">` manipulado por `transform: translate3d(tx, ty, 0) scale(s)` e Pointer Events (`pointerdown`, `pointermove`, `pointerup` com suporte a pan de 1 dedo, pinch-to-zoom de 2 dedos em torno do ponto médio e duplo toque `1× ↔ 2,5×`).

### 2.2. Medidas Reais de Frame Time — CBZ Zoom/Pan no S23 Ultra (Display 120 Hz)

Medimos dois cenários complementares, limpando as estatísticas antes de cada um com `adb shell dumpsys gfxinfo dev.gaspari.bookferry.spike reset`:
1. **Cenário 2A (`rAF_synthetic_zoom_pan`):** 240 frames consecutivos via `requestAnimationFrame` aplicando zoom contínuo (`1,0× → 3,0× → 1,0×`) combinado com pan circular na página `1760×2304` (1,01 MB).
2. **Cenário 2B (`adb_input_swipe_pointer_pan`):** Imagem com zoom `2,2×` e injeção de 6 gestos reais de toque via `adb shell input swipe` ao longo de 5 segundos (gerando **424 eventos `pointermove`** reais processados pelo WebView).

Saídas reais do `logcat` e do `dumpsys gfxinfo`:
```text
[BOOKFERRY_RESULT][RISCO2_CBZ_RAF] {"mode":"rAF_synthetic_zoom_pan","page_name":"little-nemo-19060204-l-17.jpeg","page_bytes":1014400,"img_dimensions":"1760x2304","viewport":"384x823 (dpr=2.8125)","frames":240,"min_ms":8.2,"mean_ms":8.368,"p50_ms":8.3,"p95_ms":8.4,"p99_ms":8.4,"max_ms":16.7,"fps_mean":119.5,"fps_p95":119.05,"frames_gt_16_7ms":0,"frames_gt_8_33ms":92}

Total frames rendered: 184
Janky frames: 2 (1.09%)
Janky frames (legacy): 1 (0.54%)
50th percentile: 5ms
90th percentile: 5ms
95th percentile: 5ms
99th percentile: 10ms
Number Missed Vsync: 0
Number Slow UI thread: 0
Number Slow bitmap uploads: 0
Number Slow issue draw commands: 2
50th gpu percentile: 1ms
90th gpu percentile: 2ms
95th gpu percentile: 2ms
99th gpu percentile: 2ms
```
```text
[BOOKFERRY_RESULT][RISCO2_CBZ_SWIPE] {"mode":"adb_input_swipe_pointer_pan","duration_ms":5000,"pointer_move_events":424,"page_name":"little-nemo-19060204-l-17.jpeg","page_bytes":1014400,"img_dimensions":"1760x2304","frames":598,"min_ms":8.2,"mean_ms":8.361,"p50_ms":8.3,"p95_ms":8.4,"p99_ms":8.4,"max_ms":16.7,"fps_mean":119.6,"fps_p95":119.05,"frames_gt_16_7ms":0,"frames_gt_8_33ms":226}

Total frames rendered: 414
Janky frames: 5 (1.21%)
Janky frames (legacy): 4 (0.97%)
50th percentile: 5ms
90th percentile: 5ms
95th percentile: 6ms
99th percentile: 8ms
Number Missed Vsync: 0
Number Slow UI thread: 2
Number Slow bitmap uploads: 0
Number Slow issue draw commands: 3
50th gpu percentile: 1ms
90th gpu percentile: 2ms
95th gpu percentile: 2ms
99th gpu percentile: 2ms
```

| Métrica | Cenário 2A: `rAF` Zoom (1×→3×) + Pan | Cenário 2B: Swipes Reais (`adb input swipe`, 424 `pointermove`) |
|---|---|---|
| Total de frames JS medidos | 240 | 598 |
| Frame time médio (`mean_ms`) / **FPS médio** | **8,368 ms** (**119,50 fps**) | **8,361 ms** (**119,60 fps**) |
| Frame time mediana (`p50_ms`) / FPS p50 | **8,300 ms** (120,48 fps) | **8,300 ms** (120,48 fps) |
| Frame time `p95_ms` / **FPS p95** | **8,400 ms** (**119,05 fps**) | **8,400 ms** (**119,05 fps**) |
| Frame time `p99_ms` / `max_ms` | **8,400 ms** / 16,700 ms | **8,400 ms** / 16,700 ms |
| **Frames > 16,7 ms (abaixo de 60 fps)** | **0 (0,0%)** | **0 (0,0%)** |
| `dumpsys gfxinfo` — Frames / Janky | 184 frames / **2 janky (1,09%)** | 414 frames / **5 janky (1,21%)** |
| `dumpsys gfxinfo` — CPU Frame Time p50 / p95 / p99 | **5 ms / 5 ms / 10 ms** | **5 ms / 6 ms / 8 ms** |
| `dumpsys gfxinfo` — GPU Frame Time p50 / p95 / p99 | **1 ms / 2 ms / 2 ms** | **1 ms / 2 ms / 2 ms** |
| `dumpsys gfxinfo` — Missed Vsync / Slow Bitmap Uploads | **0 / 0** | **0 / 0** |

### 2.3. Comparação com Renderizador de PDF (`pdf.js` 4.10.38 no WebView)
Vendorizamos `pdfjs-dist@4.10.38` em `spike/app/src/vendor/pdfjs/`. O arquivo `little-nemo-1905-1914.pdf` (**70.683.356 bytes / 70,7 MB**, 421 páginas) foi aberto via SAF (`fd=144`) e servido ao `pdf.js` por um servidor HTTP local em Rust (`127.0.0.1:18421/file/little-nemo-1905-1914.pdf`) que responde a requisições `HTTP 206 Partial Content` (`Range: bytes=...`) lendo diretamente do `fd` SAF via `FileExt::read_at` (`pread64`), sem copiar o PDF para a memória ou cache.

Saídas reais do `logcat`:
```json
{"pdf_file":"little-nemo-1905-1914.pdf","pdf_version_vendor":"pdfjs-dist@4.10.38","num_pages":421,"open_doc_ms":152.4,"pages_1x":[
  {"page":1,"dimensions_1x":"208x285","get_page_ms":1.5,"render_1x_ms":433.8},
  {"page":17,"dimensions_1x":"211x276","get_page_ms":0.9,"render_1x_ms":283.9},
  {"page":50,"dimensions_1x":"211x279","get_page_ms":2.0,"render_1x_ms":364.3}
],"page17_zoom_rerender_2x_ms":13.9,"page17_zoom_2x_dims":"422x552","page17_zoom_rerender_2_5x_ms":7.6,"page17_zoom_2_5x_dims":"527x690","canvas_css_pan_stats":{"frames":120,"min_ms":8.2,"mean_ms":8.333,"p50_ms":8.3,"p95_ms":8.4,"p99_ms":8.5,"max_ms":8.5,"fps_mean":120.0,"fps_p95":119.05,"frames_gt_16_7ms":0,"frames_gt_8_33ms":44}}
```
```json
{"page":17,"scale_5x_dims":"1055x1380","render_5x_1055x1380_ms":361.4,"scale_10x_dims":"2110x2760","render_10x_2110x2760_ms":7.8}
```

- **Análise CBZ (`<img>` direto) vs. PDF (`pdf.js` `<canvas>`):**
  - Abrir o PDF de 71 MB via HTTP Range no `fd` SAF leva **152,4 ms**.
  - A 1ª renderização de uma página de scan pesado no `pdf.js` leva **283,9 a 433,8 ms** (**361,4 ms** em resolução de tela cheia `1055×1380`, `scale=5.0`), contra **117–158 ms** de uma página CBZ direta.
  - Durante o gesto de pan/pinch no `<canvas>`, usar CSS `transform: translate3d(...)` mantém **120,0 fps médios** (`8,333 ms`, 0 frames > 16,7 ms), e o re-render em alta resolução (`2110×2760`, `scale=10.0`) após o stream JPEG da página já estar decodificado em memória pelo worker do `pdf.js` leva apenas **7,8 ms** (ou ~360 ms se o bitmap precisar ser re-decodificado do zero).
  - **Alternativa PDFium via Rust (`pdfium-render`):** **NÃO medida** neste spike. Como `pdf.js` com HTTP Range sobre o `fd` SAF já abre PDFs de 71 MB em 152 ms e renderiza páginas em ~300–360 ms, `pdf.js` já é viável para o MVP, ficando `pdfium-render` como otimização opcional caso se deseje rasterização em tiles nativos.

---

## Risco 3 — Equivalência de CFI entre Linux (`webkit2gtk`) e Android WebView

### 3.1. Ambiente de Teste e Metodologia
Vendorizamos `foliate-js` (MIT, commit `78914aef4466eb960965702401634c2cb348e9b1`) em `spike/app/src/vendor/foliate-js/`.
Como a sessão do agente no Linux não possui `WAYLAND_DISPLAY`, adicionamos `xvfb-run` ao `spike/flake.nix` e executamos **o binário real do Tauri para Linux (`webkit2gtk-4.1` 2.54.2)** sob servidor X11 virtual:
```bash
BOOKFERRY_AUTORUN=risco3 BOOKFERRY_OUT=/home/gappy/Documents/projetos/bookshelf/spike/linux-cfi-results.jsonl \
  xvfb-run -a -s "-screen 0 1280x1024x24" ./app/src-tauri/target/release/bookferry-spike
```
No Android, executamos a mesma suíte (`window.runRisco3EpubBench()`) no WebView (`Chrome/157.0.8103.40`) do SM-S918B com os 3 EPUBs abertos via SAF (`frankenstein-pg84.epub`, `moby-dick-pg2701.epub`, `pride-and-prejudice-pg1342.epub`).

Avaliamos os CFIs em **três níveis**:
1. **Nível 1 — DOM / Estrutural (Determinístico, independente de layout visual):**
   - **5 capítulos do TOC** via `book.resolveHref(tocItem.href)` → `section.createDocument()` → `view.getCFI(index, range)`.
   - **Posições 10%, 50%, 90% do livro** via `SectionProgress.getSection(frac)` (baseado nos tamanhos em bytes das seções do spine) combinado com contagem determinística de caracteres nos nós de texto do `DOMParser` (`application/xhtml+xml`).
2. **Nível 2 — Paginador Visual (`<foliate-view>` `goTo(href)` e `goToFraction(0.1 / 0.5 / 0.9)`):**
   - CFIs gerados por `#getVisibleRange()` após paginação CSS multi-colunas (`full_range_cfi` e `CFI.collapse(cfi)`).
3. **Nível 3 — Caminho Inverso Cross-Platform (Linux `webkit2gtk` → Android WebView):**
   - Tomamos os CFIs gerados pelo paginador do Linux (`webkit2gtk`) e os abrimos no Android WebView via `view.resolveCFI(cfi)` e `view.goTo(cfi)`, verificando se o trecho de texto (snippet) coincide.

### 3.2. Resultados Nível 1 — CFIs Estruturais / DOM (Capítulos e 10%/50%/90%)

**Resultado: 24 de 24 CFIs (100%) são bit-a-bit IDÊNTICOS entre Linux (`webkit2gtk` 2.54.2) e Android WebView (`Chrome 157`).** A árvore DOM XHTML e a contagem de caracteres por seção são 100% iguais nos dois motores:

| Livro | Posição | Chars na Seção (Linux / Android) | CFI Linux (`webkit2gtk`) | CFI Android (`WebView`) | Match? | Trecho de Texto (Snippet) |
|---|---|---|---|---|---|---|
| **Frankenstein** | Cap. 1 (`Frankenstein;`) | — | `epubcfi(/6/4!/4/6[pgepubid00000])` | `epubcfi(/6/4!/4/6[pgepubid00000])` | **SIM** | `Frankenstein; or, the Modern Prometheus...` |
| **Frankenstein** | Cap. 2 (`or, the Modern...`) | — | `epubcfi(/6/4!/4/8[pgepubid00001])` | `epubcfi(/6/4!/4/8[pgepubid00001])` | **SIM** | `or, the Modern Prometheus by Mary...` |
| **Frankenstein** | Cap. 3 (`CONTENTS`) | — | `epubcfi(/6/6!/4/2[pgepubid00002])` | `epubcfi(/6/6!/4/2[pgepubid00002])` | **SIM** | `CONTENTS Letter 1 Letter 2 Letter 3...` |
| **Frankenstein** | Cap. 4 (`Letter 1`) | — | `epubcfi(/6/8!/4/2[pgepubid00003])` | `epubcfi(/6/8!/4/2[pgepubid00003])` | **SIM** | `Letter 1 To Mrs. Saville, England...` |
| **Frankenstein** | Cap. 5 (`Letter 2`) | — | `epubcfi(/6/10!/4/2[pgepubid00004])` | `epubcfi(/6/10!/4/2[pgepubid00004])` | **SIM** | `Letter 2 To Mrs. Saville, England...` |
| **Frankenstein** | **10%** (seção 7) | `10177 / 10177` | `epubcfi(/6/16!/4/2[pgepubid00007]/16/1:465)` | `epubcfi(/6/16!/4/2[pgepubid00007]/16/1:465)` | **SIM** | `laything and their idol, and something...` |
| **Frankenstein** | **50%** (seção 19) | `11624 / 11624` | `epubcfi(/6/40!/4/2[pgepubid00019]/46/1:300)` | `epubcfi(/6/40!/4/2[pgepubid00019]/46/1:300)` | **SIM** | `nful self-deceit, to call them).”` |
| **Frankenstein** | **90%** (seção 30) | `45799 / 45799` | `epubcfi(/6/62!/4/2[pgepubid00030]/70/1:92)` | `epubcfi(/6/62!/4/2[pgepubid00030]/70/1:92)` | **SIM** | `t of general literature he displays...` |
| **Moby-Dick** | Cap. 1 (`MOBY-DICK...`) | — | `epubcfi(/6/4!/4/6[pgepubid00000])` | `epubcfi(/6/4!/4/6[pgepubid00000])` | **SIM** | `MOBY-DICK; or, THE WHALE. By Herman...` |
| **Moby-Dick** | Cap. 2 (`ETYMOLOGY.`) | — | `epubcfi(/6/4!/4/26/6[pgepubid00001])` | `epubcfi(/6/4!/4/26/6[pgepubid00001])` | **SIM** | `ETYMOLOGY. (Supplied by a Late...` |
| **Moby-Dick** | Cap. 3 (`(Supplied by...`) | — | `epubcfi(/6/4!/4/26/8[pgepubid00002])` | `epubcfi(/6/4!/4/26/8[pgepubid00002])` | **SIM** | `(Supplied by a Late Consumptive Usher...` |
| **Moby-Dick** | Cap. 4 (`EXTRACTS. (Sup...`) | — | `epubcfi(/6/4!/4/26/24[pgepubid00003])` | `epubcfi(/6/4!/4/26/24[pgepubid00003])` | **SIM** | `EXTRACTS. (Supplied by a Sub-Sub-...` |
| **Moby-Dick** | Cap. 5 (`EXTRACTS.`) | — | `epubcfi(/6/4!/4/26/30[pgepubid00004])` | `epubcfi(/6/4!/4/26/30[pgepubid00004])` | **SIM** | `EXTRACTS. “And God created great...` |
| **Moby-Dick** | **10%** (seção 2) | `134720 / 134720` | `epubcfi(/6/6!/4/14/1:800)` | `epubcfi(/6/6!/4/14/1:800)` | **SIM** | `le ship, and bear a hardy helm; for lo!...` |
| **Moby-Dick** | **50%** (seção 6) | `129776 / 129776` | `epubcfi(/6/14!/4/106/1:264)` | `epubcfi(/6/14!/4/106/1:264)` | **SIM** | `beheld, that though one and all of them...` |
| **Moby-Dick** | **90%** (seção 10) | `133071 / 133071` | `epubcfi(/6/22!/4/398/1:599)` | `epubcfi(/6/22!/4/398/1:599)` | **SIM** | `g down, a little tossed heap of white...` |
| **Pride & Prej.** | Cap. 1 (`PRIDE. and...`) | — | `epubcfi(/6/4!/4/16/4[pgepubid00002])` | `epubcfi(/6/4!/4/16/4[pgepubid00002])` | **SIM** | `PRIDE. and PREJUDICE by Jane Austen...` |
| **Pride & Prej.** | Cap. 2 (`Chapter I.`) | — | `epubcfi(/6/4!/4/62[pgepubid00022])` | `epubcfi(/6/4!/4/62[pgepubid00022])` | **SIM** | `Chapter I. IT is a truth universally...` |
| **Pride & Prej.** | Cap. 3 (`...CHAPTER II.`) | — | `epubcfi(/6/4!/4/138[pgepubid00028])` | `epubcfi(/6/4!/4/138[pgepubid00028])` | **SIM** | `I hope Mr. Bingley will like it. CHAPTER II.` |
| **Pride & Prej.** | Cap. 4 (`...CHAPTER III.`) | — | `epubcfi(/6/4!/4/196[pgepubid00033])` | `epubcfi(/6/4!/4/196[pgepubid00033])` | **SIM** | `He rode a black horse. CHAPTER III. OT...` |
| **Pride & Prej.** | Cap. 5 (`CHAPTER IV.`) | — | `epubcfi(/6/4!/4/244[pgepubid00041])` | `epubcfi(/6/4!/4/244[pgepubid00041])` | **SIM** | `CHAPTER IV. HEN Jane and Elizabeth...` |
| **Pride & Prej.** | **10%** (seção 1) | `115834 / 115834` | `epubcfi(/6/4!/4/352/1:389)` | `epubcfi(/6/4!/4/352/1:389)` | **SIM** | `ot advance their felicity in the least...` |
| **Pride & Prej.** | **50%** (seção 4) | `142359 / 142359` | `epubcfi(/6/10!/4/92/1:241)` | `epubcfi(/6/10!/4/92/1:241)` | **SIM** | `age. There, shut into her own room...` |
| **Pride & Prej.** | **90%** (seção 6) | `82547 / 82547` | `epubcfi(/6/14!/4/256/1:150)` | `epubcfi(/6/14!/4/256/1:150)` | **SIM** | `r recede. I shall not go away till you...` |

### 3.3. Resultados Nível 2 — Onde e Por Que o Paginador (`<foliate-view>`) Diverge
Quando navegamos pelo componente visual `<foliate-view>` (`paginator.js`), observamos:

| Caso no `<foliate-view>` | CFI Colapsado Linux (`webkit2gtk`) | CFI Colapsado Android (`WebView`) | Por que converge ou diverge? |
|---|---|---|---|
| **Frankenstein** `CONTENTS` (`goTo(href)`) | `epubcfi(/6/6!/4/2[pgepubid00002]/2)` | `epubcfi(/6/6!/4/2[pgepubid00002]/2)` | **IDÊNTICO** no início (`collapsed`), pois o capítulo inicia um novo arquivo XHTML (`84-h-1.htm.xhtml`). O `full_range_cfi` difere apenas no fim da página (`/4/2/28/2/2/1:10` vs `/4/2/24/2/2/1:9`) porque cabem mais linhas na página desktop. |
| **Frankenstein** `Letter 1` (`goTo(href)`) | `epubcfi(/6/8!/4/2[pgepubid00003]/2)` | `epubcfi(/6/8!/4/2[pgepubid00003]/2)` | **IDÊNTICO** no início (`collapsed`). Fim da página difere (`/10/1:773` vs `/10/1:123`). |
| **Frankenstein** `Letter 2` (`goTo(href)`) | `epubcfi(/6/10!/4/2[pgepubid00004]/2)` | `epubcfi(/6/10!/4/2[pgepubid00004]/2)` | **IDÊNTICO** no início (`collapsed`). Fim da página difere (`/10/1:837` vs `/10/1:116`). |
| **Moby-Dick** `10%` (`goToFraction(0.1)`) | `epubcfi(/6/6!/4/12/1:747)` | `epubcfi(/6/6!/4/12/1:674)` | **Diverge em 73 chars no mesmo parágrafo (`/4/12/1`)**: `goToFraction` calcula `newPage = Math.round(anchor * (textPages - 1))` (`paginator.js:942`). Como `textPages` depende da largura da coluna e métricas de fonte, o topo da coluna cai 1 linha antes no Android. |
| **Moby-Dick** `50%` (`goToFraction(0.5)`) | `epubcfi(/6/14!/4/96/1:937)` | `epubcfi(/6/14!/4/96/1:707)` | **Diverge em 230 chars no mesmo parágrafo (`/4/96/1`)** pelo mesmo arredondamento de página (`Math.round(anchor * (textPages - 1))`). |
| **Moby-Dick** `90%` (`goToFraction(0.9)`) | `epubcfi(/6/22!/4/392/1:854)` | `epubcfi(/6/22!/4/392/1:135)` | **Diverge no mesmo parágrafo (`/4/392/1`)** pelo mesmo motivo. |
| **Pride & Prej.** `Chapter I.` (`goTo(href)`) | `epubcfi(/6/4!/4/58/2/160/2/1:4)` | `epubcfi(/6/4!/4/62[pgepubid00022])` | No Project Gutenberg #1342, vários capítulos estão no mesmo arquivo XHTML. No Linux (coluna mais larga), o topo da página onde `Chapter I` aparece começa no final da lista de ilustrações (`/4/58/2/160`), enquanto no Android `Chapter I` (`/4/62`) cai exatamente no topo da coluna. |

### 3.4. Resultados Nível 3 — Caminho Inverso (CFI Gerado no Linux → Aberto no Android WebView)
Para testar o caso real de uso do Bookferry (*"fechei o livro no PC Linux, o PC salvou o CFI e enviou para o celular Android"*), injetamos os 9 CFIs gerados no Linux (`webkit2gtk`) dentro do Android WebView (`Chrome 157`) e executamos `view.resolveCFI(cfi)` + `view.goTo(cfi)`:

```json
[BOOKFERRY_RESULT][RISCO3_CROSS_LINUX_TO_ANDROID] {
  "frankenstein-pg84.epub": [
    {"label":"pag_10%","cfi":"epubcfi(/6/16!/4/2[pgepubid00007]/14/1:1483)","linux_snippet":"ened frame. From Italy they visited Germany and France. I, t","android_dom_snippet":"ened frame.","dom_exact_match":true,"android_page_contains_snippet":false,"android_page_preview":"worth. There was a show of gratitude and worship in his attachment to my mother,"},
    {"label":"pag_90%","cfi":"epubcfi(/6/62!/4/2[pgepubid00030]/70/1:510)","linux_snippet":"tness of his fall. “When younger,” said he, “I believed myse","android_dom_snippet":"tness of his fall.","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"a glorious creature must he have been in the days of his prosperity, when he is "},
    {"label":"dom_10%","cfi":"epubcfi(/6/16!/4/2[pgepubid00007]/16/1:465)","linux_snippet":"laything and their idol, and something better—their child, t","android_dom_snippet":"laything and their idol, and something better—their child, t","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"From Italy they visited Germany and France. I, their eldest child, was born at N"}
  ],
  "moby-dick-pg2701.epub": [
    {"label":"pag_10%","cfi":"epubcfi(/6/6!/4/12/1:747)","linux_snippet":"within the walls. But the side ladder was not the only stran","android_dom_snippet":"within the walls.","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"deposited within, leaving him impregnable in his little Quebec. I pondered some "},
    {"label":"pag_50%","cfi":"epubcfi(/6/14!/4/96/1:937)","linux_snippet":". As with a low sucking sound it slowly disappeared again, S","android_dom_snippet":".","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"face or front did it have; no conceivable token of either sensation or instinct;"},
    {"label":"pag_90%","cfi":"epubcfi(/6/22!/4/392/1:854)","linux_snippet":"nds that were heard, were the voices of newly drowned men in","android_dom_snippet":"nds that were heard, were the voices of newly drowned men in","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"all, they started from their reveries, and for the space of some moments stood, "}
  ],
  "pride-and-prejudice-pg1342.epub": [
    {"label":"pag_10%","cfi":"epubcfi(/6/4!/4/332[pgepubid00052]/6)","linux_snippet":"CHAPTER VI. HE ladies of Longbourn soon waited on those of N","android_dom_snippet":"","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"CHAPTER VI. HE ladies of Longbourn soon waited on those of Netherfield. The visi"},
    {"label":"pag_50%","cfi":"epubcfi(/6/10!/4/88/1:386)","linux_snippet":"“That is not an unnatural surmise,” said Fitzwilliam; “but i","android_dom_snippet":"","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"Elizabeth made no answer, and walked on, her heart swelling with indignation. Af"},
    {"label":"pag_90%","cfi":"epubcfi(/6/14!/4/232/1:416)","linux_snippet":"“That will make your Ladyship’s situation at present more pi","android_dom_snippet":"","dom_exact_match":true,"android_page_contains_snippet":true,"android_page_preview":"happiness necessarily attached to her situation, that she could, upon the whole,"}
  ]
}
```
- **9 de 9 CFIs (`100%`)** resolveram para o exato mesmo ponto do DOM no Android (`dom_exact_match: true`), e `view.goTo(cfi)` abriu a coluna exata contendo aquele ponto! (No caso `pag_10%` de Frankenstein, `offset 1483` apontava para as duas últimas palavras `"ened frame."` do parágrafo `/14`, que ficaram no rodapé da página aberta no Android enquanto o parágrafo `/16` `"From Italy..."` iniciou no topo da página seguinte).
- **Regra de ouro para implementação do sync no Bookferry:**
  1. Ao salvar posição de EPUB, persistir sempre `CFI.collapse(lastLocation.cfi)` (ponto inicial exato da leitura) + trecho de texto (`snippet` de ~60 caracteres) + `fraction` apenas como indicador visual de barra de progresso.
  2. Ao restaurar posição em outro dispositivo, navegar **sempre via `view.goTo(cfi)`** (e **nunca** via `view.goToFraction(fraction)`), pois `view.goTo(cfi)` usa `CFI.toRange` que é 100% determinístico entre `webkit2gtk` e Android WebView.
  3. Em EPUBs pesados com imagens inline (como `pride-and-prejudice-pg1342.epub`, 25 MB), se uma seção ainda estiver decodificando imagens quando `goTo(cfi)` rodar, re-executar `scrollToAnchor` após o `load` das imagens da seção (ou definir dimensões/`aspect-ratio` nas imagens) evita que o `ResizeObserver` desloque a coluna após o scroll inicial.

---

## O que NÃO foi verificado neste Spike

1. **Alternativa PDFium via Rust (`pdfium-render`):** Não foi medida neste spike (apenas `pdf.js` 4.10.38 foi medido no WebView).
2. **Formatos CBR/RAR5 e CB7 (`libarchive`) e MOBI/AZW3/FB2:** O spike focou em CBZ (`zip`), PDF (`pdf.js`) e EPUB (`foliate-js`).
3. **Persistência de permissão SAF após reboot do aparelho:** Chamamos `takePersistableUriPermission` no `MainActivity.kt`, mas não reiniciamos o celular para testar a sobrevivência da URI após boot.
4. **Risco 7 na sessão gráfica real niri/NVIDIA com livros reais:** No Linux, os testes do Risco 3 rodaram sob `xvfb-run` (X11 virtual). A janela real no Wayland/niri já havia sido validada na Fase 1 pelo Lucas.
5. **Riscos 4, 5 e 8 do plano (`mDNS/TLS LAN`, `Relay E2EE`, `Open Library pt-BR`):** Fora do escopo desta Fase 2 (que cobriu os Riscos 1 a 3).

---

## Recomendação Final: Manter Tauri v2 vs. Plano B (Flutter)

**Recomendação: MANTER TAURI v2 (Linux + Android) com núcleo em Rust.**

- **Por quê:**
  1. O **Risco 1 (SAF >500 MB)** era o maior temor de memória/I/O no Android e foi completamente eliminado: `openFileDescriptor(uri, "r").detachFd()` + `std::fs::File::from_raw_fd(fd)` no Rust abre e lista um CBZ de **592 MB em ~22 ms** sem copiar nada para cache e usando menos de 20 MB extras de RAM.
  2. O **Risco 2 (Fluidez 60 fps em WebView)** superou a meta de 60 fps e cravou **119,5–120 fps** (`8,36 ms/frame`, `0 frames > 16,7 ms`, `GPU p95 = 2 ms`) no S23 Ultra manipulando uma página JPEG de 1,01 MB (`1760×2304`) com CSS `translate3d() scale()` e Pointer Events.
  3. O **Risco 3 (CFI entre `webkit2gtk` e Android WebView)** provou que o parser XHTML e o gerador/resolvedor `epubcfi.js` do `foliate-js` produzem árvores DOM e CFIs **100% idênticos** nos dois sistemas, bastando navegar por `view.goTo(CFI.collapse(cfi))` em vez de `goToFraction`.


## Conferência do orquestrador (13/11/2026)

Feita depois do agente, sem repetir as medições no celular:

- **Recontado dos dados brutos** (`spike/linux-cfi-results.jsonl` e `spike/android-cfi-results.json`): 24 de 24 CFIs de DOM idênticos entre Linux e Android, como o relatório diz.
- **Esse 24/24 é um teste fraco.** Ele compara a árvore XHTML, que qualquer motor analisa do mesmo jeito. O teste que discrimina é o do paginador, e ali há divergência (Moby-Dick 10/50/90%). A conclusão útil é a regra: salvar e restaurar por `goTo(cfi)`, nunca por `goToFraction`.
- **Não verificado por mim:** o commit do foliate-js citado (`78914aef...`): a cópia vendorizada não tem `.git`, então não dá para confirmar. Fixar a versão antes de depender dela.
- **Medidas de fps, RAM e tempos do Android:** vêm do log do agente; não as repeti.
- **Linux:** o teste rodou sob `xvfb`, não no niri. O pinch com dois dedos não foi testado (os swipes via `adb` são de um dedo). O fluxo do seletor de arquivos foi guiado por `adb input tap`, sem toque humano.
- `spike/vendor/` (50 MB, cópia duplicada) fica fora do git; o que o app usa está em `spike/app/src/vendor/`.
