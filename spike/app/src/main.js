const invoke = window.__TAURI__.core.invoke;
const outEl = document.getElementById("out");

let envInfo = { os: "unknown", arch: "unknown", server_port: 18421 };

function appendOut(msg) {
  const line = `[${new Date().toISOString().slice(11, 23)}] ${msg}`;
  outEl.textContent = line + "\n" + outEl.textContent.slice(0, 16000);
}

async function logResult(tag, dataObj) {
  const jsonStr = typeof dataObj === "string" ? dataObj : JSON.stringify(dataObj);
  appendOut(`${tag}: ${jsonStr.slice(0, 300)}${jsonStr.length > 300 ? "..." : ""}`);
  if (window.AndroidSaf && typeof window.AndroidSaf.log === "function") {
    window.AndroidSaf.log(`[BOOKFERRY_RESULT][${tag}] ${jsonStr}`);
  }
  try {
    await invoke("log_result", { tag, payload: JSON.stringify({ tag, data: dataObj }) });
  } catch (e) {
    console.error("invoke log_result err:", e);
  }
}

// ============================================================================
// RISCO 1: SAF (ACTION_OPEN_DOCUMENT) -> fd -> Rust (Zero Copy)
// ============================================================================

async function benchmarkCbzFromSafItem(item) {
  appendOut(`Iniciando benchmark CBZ via SAF fd=${item.fd} (${item.name}, ${item.size} bytes)...`);
  // Escolhe a maior pagina (~1 MB: little-nemo-19060204-l-17.jpeg, indice 16 no original ou page17.jpeg no sintetico)
  const targetName = item.name.includes("synthetic")
    ? "page17.jpeg"
    : "little-nemo-19060204-l-17.jpeg";

  const runs = [];
  for (let rep = 0; rep < 3; rep++) {
    let currentItem = item;
    if (rep > 0 && window.AndroidSaf && window.AndroidSaf.reopenFd) {
      currentItem = JSON.parse(window.AndroidSaf.reopenFd(item.name));
    }
    const t0 = performance.now();
    const res = await invoke("saf_inspect_and_read_cbz", {
      fd: currentItem.fd,
      name: currentItem.name,
      pageIndex: 16,
      targetName,
    });
    const tIpc = performance.now() - t0;

    const comicImg = document.getElementById("comic-img");
    comicImg.src = res.data_url;
    await comicImg.decode();
    const tFirstPageRendered = performance.now() - t0;

    runs.push({
      rep,
      open_fd_ms: currentItem.openFdMs,
      zip_open_ms: res.zip_open_ms,
      list_natural_sort_ms: res.list_natural_sort_ms,
      read_single_page_ms: res.read_single_page_ms,
      base64_encode_ms: res.base64_encode_ms,
      total_rust_ms: res.total_rust_ms,
      ipc_roundtrip_ms: tIpc,
      first_page_rendered_ms: tFirstPageRendered,
      mem_before: res.mem_before,
      mem_after: res.mem_after,
      img_width: comicImg.naturalWidth,
      img_height: comicImg.naturalHeight,
      file_size_bytes: res.file_size_bytes || currentItem.size,
      total_zip_entries: res.total_zip_entries,
      ignored_macosx_entries: res.ignored_macosx_entries,
      valid_page_count: res.valid_page_count,
      first_5_pages: res.first_5_pages,
      last_5_pages: res.last_5_pages,
      target_page_index: res.target_page_index,
      target_page_name: res.target_page_name,
      target_page_bytes: res.target_page_bytes,
      copied_to_cache: res.copied_to_cache,
    });
  }

  const summary = {
    file_name: item.name,
    uri: item.uri,
    file_size_bytes: runs[0].file_size_bytes,
    total_zip_entries: runs[0].total_zip_entries,
    ignored_macosx_entries: runs[0].ignored_macosx_entries,
    valid_page_count: runs[0].valid_page_count,
    first_5_pages: runs[0].first_5_pages,
    last_5_pages: runs[0].last_5_pages,
    target_page_index: runs[0].target_page_index,
    target_page_name: runs[0].target_page_name,
    target_page_bytes: runs[0].target_page_bytes,
    img_dimensions: `${runs[0].img_width}x${runs[0].img_height}`,
    copied_to_cache: false,
    runs: runs.map(r => ({
      rep: r.rep,
      open_fd_ms: Number(r.open_fd_ms.toFixed(3)),
      zip_open_ms: Number(r.zip_open_ms.toFixed(3)),
      list_natural_sort_ms: Number(r.list_natural_sort_ms.toFixed(3)),
      read_single_page_ms: Number(r.read_single_page_ms.toFixed(3)),
      base64_encode_ms: Number(r.base64_encode_ms.toFixed(3)),
      total_rust_ms: Number(r.total_rust_ms.toFixed(3)),
      ipc_roundtrip_ms: Number(r.ipc_roundtrip_ms.toFixed(3)),
      first_page_rendered_ms: Number(r.first_page_rendered_ms.toFixed(3)),
      mem_before: r.mem_before,
      mem_after: r.mem_after,
    })),
  };

  await logResult("RISCO1_CBZ_SAF", summary);
  return summary;
}

window.__onSafPicked = async (items) => {
  try {
    appendOut(`SAF retornou ${items.length} arquivo(s)`);
    for (const item of items) {
      if (item.name.endsWith(".cbz")) {
        await benchmarkCbzFromSafItem(item);
      } else {
        const reg = await invoke("register_saf_file", { fd: item.fd, name: item.name });
        await logResult("SAF_FILE_REGISTERED", reg);
      }
    }
  } catch (err) {
    await logResult("SAF_ERROR", { error: String(err) });
  }
};

window.testDirectAccess = async () => {
  const files = [
    "little-nemo-1905-1914.cbz",
    "synthetic-600mb.cbz",
    "little-nemo-1905-1914.pdf",
    "frankenstein-pg84.epub",
    "moby-dick-pg2701.epub",
    "pride-and-prejudice-pg1342.epub",
  ];
  const results = [];
  for (const f of files) {
    const p = `/sdcard/Download/bookferry-fixtures/${f}`;
    try {
      const info = await invoke("try_open_direct_path", { path: p, registerAs: f });
      results.push({ file: f, ok: true, size: info.size });
    } catch (e) {
      results.push({ file: f, ok: false, error: String(e) });
    }
  }
  await logResult("DIRECT_PATH_PROBE", results);
  return results;
};

// ============================================================================
// RISCO 2: Fluidez de Zoom/Pan em WebView (CSS Transform + Pointer Events)
//          + Comparacao com PDF.js
// ============================================================================

const comicStage = document.getElementById("comic-stage");
const comicImg = document.getElementById("comic-img");
const comicHudText = document.getElementById("comic-hud-text");

let tx = 0, ty = 0, scale = 1;
const activePointers = new Map();
let lastPinchDist = null;
let lastPinchCenter = null;
let lastTapTime = 0;
let swipeCapture = null;

function applyComicTransform() {
  comicImg.style.transform = `translate3d(${tx.toFixed(2)}px, ${ty.toFixed(2)}px, 0) scale(${scale.toFixed(4)})`;
}

function fitComicToScreen() {
  const vw = window.innerWidth || 1080;
  const vh = window.innerHeight || 2340;
  const iw = comicImg.naturalWidth || 1400;
  const ih = comicImg.naturalHeight || 1900;
  scale = Math.min(vw / iw, vh / ih);
  tx = (vw - iw * scale) / 2;
  ty = (vh - ih * scale) / 2;
  applyComicTransform();
}

comicStage.addEventListener("pointerdown", (e) => {
  comicStage.setPointerCapture(e.pointerId);
  activePointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

  if (activePointers.size === 1) {
    const now = performance.now();
    if (now - lastTapTime < 280) {
      // Double tap zoom toggle around tap point
      const targetScale = scale > 1.5 ? (window.innerWidth / (comicImg.naturalWidth || 1400)) : 2.5;
      const ratio = targetScale / scale;
      tx = e.clientX - (e.clientX - tx) * ratio;
      ty = e.clientY - (e.clientY - ty) * ratio;
      scale = targetScale;
      applyComicTransform();
      lastTapTime = 0;
    } else {
      lastTapTime = now;
    }
  } else if (activePointers.size === 2) {
    const pts = Array.from(activePointers.values());
    lastPinchDist = Math.hypot(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
    lastPinchCenter = { x: (pts[0].x + pts[1].x) / 2, y: (pts[0].y + pts[1].y) / 2 };
  }
});

comicStage.addEventListener("pointermove", (e) => {
  if (!activePointers.has(e.pointerId)) return;
  const prev = activePointers.get(e.pointerId);
  const curr = { x: e.clientX, y: e.clientY };
  activePointers.set(e.pointerId, curr);

  if (swipeCapture) {
    swipeCapture.pointerEvents++;
  }

  if (activePointers.size === 1) {
    tx += curr.x - prev.x;
    ty += curr.y - prev.y;
    applyComicTransform();
  } else if (activePointers.size >= 2) {
    const pts = Array.from(activePointers.values());
    const dist = Math.hypot(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
    const center = { x: (pts[0].x + pts[1].x) / 2, y: (pts[0].y + pts[1].y) / 2 };
    if (lastPinchDist && lastPinchDist > 5) {
      const ratio = dist / lastPinchDist;
      const newScale = Math.max(0.3, Math.min(6.0, scale * ratio));
      const actualRatio = newScale / scale;
      tx = center.x - (lastPinchCenter.x - tx) * actualRatio;
      ty = center.y - (lastPinchCenter.y - ty) * actualRatio;
      scale = newScale;
      applyComicTransform();
    }
    lastPinchDist = dist;
    lastPinchCenter = center;
  }
});

function onPointerEnd(e) {
  activePointers.delete(e.pointerId);
  if (activePointers.size < 2) {
    lastPinchDist = null;
    lastPinchCenter = null;
  }
}
comicStage.addEventListener("pointerup", onPointerEnd);
comicStage.addEventListener("pointercancel", onPointerEnd);

document.getElementById("btn-close-comic").addEventListener("click", () => {
  comicStage.style.display = "none";
});

function computeFrameStats(deltas) {
  if (!deltas.length) return {};
  const sorted = [...deltas].sort((a, b) => a - b);
  const sum = sorted.reduce((a, b) => a + b, 0);
  const mean = sum / sorted.length;
  const p50 = sorted[Math.floor(sorted.length * 0.5)];
  const p95 = sorted[Math.floor(sorted.length * 0.95)];
  const p99 = sorted[Math.floor(sorted.length * 0.99)];
  const max = sorted[sorted.length - 1];
  const min = sorted[0];
  const gt16_7 = sorted.filter(d => d > 16.7).length;
  const gt8_33 = sorted.filter(d => d > 8.33).length;
  return {
    frames: sorted.length,
    min_ms: Number(min.toFixed(3)),
    mean_ms: Number(mean.toFixed(3)),
    p50_ms: Number(p50.toFixed(3)),
    p95_ms: Number(p95.toFixed(3)),
    p99_ms: Number(p99.toFixed(3)),
    max_ms: Number(max.toFixed(3)),
    fps_mean: Number((1000 / mean).toFixed(2)),
    fps_p95: Number((1000 / p95).toFixed(2)),
    frames_gt_16_7ms: gt16_7,
    frames_gt_8_33ms: gt8_33,
  };
}

async function ensureComicLoaded() {
  if (!comicImg.src || !comicImg.naturalWidth) {
    const port = envInfo.server_port || 18421;
    comicImg.src = `http://127.0.0.1:${port}/cbz-page/little-nemo-1905-1914.cbz/little-nemo-19060204-l-17.jpeg`;
    await comicImg.decode();
  }
}

window.runRisco2RafBench = async (totalFrames = 240) => {
  await ensureComicLoaded();
  comicStage.style.display = "block";
  fitComicToScreen();
  const baseScale = scale;

  const deltas = [];
  await new Promise((resolve) => {
    let frame = 0;
    let lastTs = null;
    function step(ts) {
      if (lastTs !== null) {
        deltas.push(ts - lastTs);
      }
      lastTs = ts;
      const t = frame / totalFrames;
      // Simula pinch zoom continuo (1x -> 3.0x -> 1x) + pan circular
      const z = 1.0 + 2.0 * Math.sin(t * Math.PI);
      scale = baseScale * z;
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      tx = (vw - comicImg.naturalWidth * scale) / 2 + Math.cos(t * Math.PI * 4) * 280;
      ty = (vh - comicImg.naturalHeight * scale) / 2 + Math.sin(t * Math.PI * 4) * 380;
      applyComicTransform();

      frame++;
      if (frame <= totalFrames) {
        requestAnimationFrame(step);
      } else {
        resolve();
      }
    }
    requestAnimationFrame(step);
  });

  const stats = {
    mode: "rAF_synthetic_zoom_pan",
    page_name: "little-nemo-19060204-l-17.jpeg",
    page_bytes: 1014400,
    img_dimensions: `${comicImg.naturalWidth}x${comicImg.naturalHeight}`,
    viewport: `${window.innerWidth}x${window.innerHeight} (dpr=${window.devicePixelRatio})`,
    ...computeFrameStats(deltas),
  };
  comicHudText.textContent = `rAF: mean=${stats.mean_ms}ms (${stats.fps_mean}fps) p95=${stats.p95_ms}ms >16.7ms=${stats.frames_gt_16_7ms}`;
  await logResult("RISCO2_CBZ_RAF", stats);
  return stats;
};

window.startRisco2SwipeCapture = async (durationMs = 5000) => {
  await ensureComicLoaded();
  comicStage.style.display = "block";
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  scale = 2.2;
  tx = (vw - comicImg.naturalWidth * scale) / 2;
  ty = (vh - comicImg.naturalHeight * scale) / 2;
  applyComicTransform();

  const deltas = [];
  swipeCapture = { pointerEvents: 0 };
  const tStart = performance.now();

  await new Promise((resolve) => {
    let lastTs = null;
    function loop(ts) {
      if (lastTs !== null) {
        deltas.push(ts - lastTs);
      }
      lastTs = ts;
      if (ts - tStart < durationMs) {
        requestAnimationFrame(loop);
      } else {
        resolve();
      }
    }
    requestAnimationFrame(loop);
  });

  const pointerEvents = swipeCapture.pointerEvents;
  swipeCapture = null;
  const stats = {
    mode: "adb_input_swipe_pointer_pan",
    duration_ms: durationMs,
    pointer_move_events: pointerEvents,
    page_name: "little-nemo-19060204-l-17.jpeg",
    page_bytes: 1014400,
    img_dimensions: `${comicImg.naturalWidth}x${comicImg.naturalHeight}`,
    ...computeFrameStats(deltas),
  };
  comicHudText.textContent = `Swipe: ptr=${pointerEvents} mean=${stats.mean_ms}ms (${stats.fps_mean}fps) p95=${stats.p95_ms}ms`;
  await logResult("RISCO2_CBZ_SWIPE", stats);
  comicStage.style.display = "none";
  return stats;
};

window.runRisco2PdfBench = async () => {
  try {
    appendOut("Iniciando benchmark PDF.js com little-nemo-1905-1914.pdf (71 MB)...");
    const pdfjsLib = await import("./vendor/pdfjs/pdf.min.mjs");
    pdfjsLib.GlobalWorkerOptions.workerSrc = new URL("./vendor/pdfjs/pdf.worker.min.mjs", window.location.href).href;

    const port = envInfo.server_port || 18421;
    const pdfUrl = `http://127.0.0.1:${port}/file/little-nemo-1905-1914.pdf`;

    const tOpen0 = performance.now();
    const loadingTask = pdfjsLib.getDocument({
      url: pdfUrl,
      disableAutoFetch: true,
      disableStream: false,
      rangeChunkSize: 262144,
    });
    const pdfDoc = await loadingTask.promise;
    const openDocMs = performance.now() - tOpen0;

    const canvas = document.getElementById("pdf-canvas");
    const ctx = canvas.getContext("2d");

    const pageTimings = [];
    for (const pageNum of [1, 17, 50]) {
      const tP0 = performance.now();
      const page = await pdfDoc.getPage(pageNum);
      const getPageMs = performance.now() - tP0;

      const vp1 = page.getViewport({ scale: 1.0 });
      canvas.width = Math.floor(vp1.width);
      canvas.height = Math.floor(vp1.height);

      const tR1 = performance.now();
      await page.render({ canvasContext: ctx, viewport: vp1 }).promise;
      const render1xMs = performance.now() - tR1;

      pageTimings.push({
        page: pageNum,
        dimensions_1x: `${canvas.width}x${canvas.height}`,
        get_page_ms: Number(getPageMs.toFixed(2)),
        render_1x_ms: Number(render1xMs.toFixed(2)),
      });
    }

    // Teste de zoom re-render na pagina 17 (1.0x -> 2.0x e 2.5x)
    const page17 = await pdfDoc.getPage(17);
    const vp2 = page17.getViewport({ scale: 2.0 });
    canvas.width = Math.floor(vp2.width);
    canvas.height = Math.floor(vp2.height);
    const tZoom2 = performance.now();
    await page17.render({ canvasContext: ctx, viewport: vp2 }).promise;
    const zoomRerender2xMs = performance.now() - tZoom2;

    const vp25 = page17.getViewport({ scale: 2.5 });
    canvas.width = Math.floor(vp25.width);
    canvas.height = Math.floor(vp25.height);
    const tZoom25 = performance.now();
    await page17.render({ canvasContext: ctx, viewport: vp25 }).promise;
    const zoomRerender2_5xMs = performance.now() - tZoom25;

    // CSS transform pan do canvas ja renderizado (120 frames)
    const panDeltas = [];
    await new Promise((resolve) => {
      let f = 0;
      let last = null;
      function step(ts) {
        if (last !== null) panDeltas.push(ts - last);
        last = ts;
        const dx = Math.cos((f / 120) * Math.PI * 2) * 120;
        const dy = Math.sin((f / 120) * Math.PI * 2) * 120;
        canvas.style.transform = `translate3d(${dx.toFixed(1)}px, ${dy.toFixed(1)}px, 0)`;
        f++;
        if (f <= 120) requestAnimationFrame(step);
        else resolve();
      }
      requestAnimationFrame(step);
    });
    canvas.style.transform = "";

    const result = {
      pdf_file: "little-nemo-1905-1914.pdf",
      pdf_version_vendor: "pdfjs-dist@4.10.38",
      num_pages: pdfDoc.numPages,
      open_doc_ms: Number(openDocMs.toFixed(2)),
      pages_1x: pageTimings,
      page17_zoom_rerender_2x_ms: Number(zoomRerender2xMs.toFixed(2)),
      page17_zoom_2x_dims: `${ Math.floor(vp2.width) }x${ Math.floor(vp2.height) }`,
      page17_zoom_rerender_2_5x_ms: Number(zoomRerender2_5xMs.toFixed(2)),
      page17_zoom_2_5x_dims: `${ Math.floor(vp25.width) }x${ Math.floor(vp25.height) }`,
      canvas_css_pan_stats: computeFrameStats(panDeltas),
    };
    await logResult("RISCO2_PDFJS", result);
    return result;
  } catch (err) {
    await logResult("RISCO2_PDFJS_ERROR", { error: String(err), stack: err?.stack });
    throw err;
  }
};

// ============================================================================
// RISCO 3: Equivalencia de CFI entre Linux (webkit2gtk) e Android WebView
// ============================================================================

function cleanSnippet(str, maxLen = 80) {
  return (str || "").replace(/\s+/g, " ").trim().slice(0, maxLen);
}

function getTextNodesIn(root) {
  const nodes = [];
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const parentTag = node.parentNode?.localName?.toLowerCase();
      if (parentTag === "script" || parentTag === "style") return NodeFilter.FILTER_REJECT;
      return node.nodeValue && node.nodeValue.length > 0
        ? NodeFilter.FILTER_ACCEPT
        : NodeFilter.FILTER_SKIP;
    },
  });
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    nodes.push(n);
  }
  return nodes;
}

function rangeAtCharFraction(doc, frac) {
  const nodes = getTextNodesIn(doc.body || doc.documentElement);
  const totalChars = nodes.reduce((acc, n) => acc + n.nodeValue.length, 0);
  const targetChar = Math.max(0, Math.min(totalChars - 1, Math.floor(totalChars * frac)));
  let sum = 0;
  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i];
    const len = n.nodeValue.length;
    if (sum + len > targetChar || i === nodes.length - 1) {
      const offset = Math.min(len, Math.max(0, targetChar - sum));
      const collapsedRange = doc.createRange();
      collapsedRange.setStart(n, offset);
      collapsedRange.setEnd(n, offset);

      // Extract 80-char snippet starting from this exact node/offset
      let snippet = n.nodeValue.slice(offset);
      for (let j = i + 1; j < nodes.length && snippet.length < 120; j++) {
        snippet += nodes[j].nodeValue;
      }
      return { collapsedRange, snippet: cleanSnippet(snippet, 80), totalChars, targetChar };
    }
    sum += len;
  }
  return null;
}

function extractSnippetFromResolvedAnchor(doc, anchorFn) {
  const target = anchorFn(doc);
  let startNode, startOffset;
  if (target instanceof Range || (target && typeof target.startContainer === "object")) {
    startNode = target.startContainer;
    startOffset = target.startOffset;
  } else if (target && target.nodeType) {
    startNode = target;
    startOffset = 0;
  } else {
    return "";
  }

  // Walk text nodes from startNode/startOffset
  const allText = getTextNodesIn(doc.body || doc.documentElement);
  let collecting = false;
  let acc = "";
  if (startNode.nodeType === 3) {
    for (const tn of allText) {
      if (tn === startNode) {
        collecting = true;
        acc += tn.nodeValue.slice(startOffset);
      } else if (collecting) {
        acc += tn.nodeValue;
      }
      if (acc.length >= 120) break;
    }
  } else {
    // Element node: find first text node inside or after it
    const subNodes = getTextNodesIn(startNode);
    if (subNodes.length > 0) {
      const firstTn = subNodes[0];
      for (const tn of allText) {
        if (tn === firstTn) collecting = true;
        if (collecting) acc += tn.nodeValue;
        if (acc.length >= 120) break;
      }
    } else {
      acc = startNode.textContent || "";
    }
  }
  return cleanSnippet(acc, 80);
}

function flattenToc(items) {
  const out = [];
  for (const item of items || []) {
    out.push(item);
    if (item.subitems?.length) {
      out.push(...flattenToc(item.subitems));
    }
  }
  return out;
}

window.runRisco3EpubBench = async (foreignCfisToVerify = null) => {
  try {
    appendOut("Iniciando Risco 3 (foliate-js CFI em 3 EPUBs)...");
    await import("./vendor/foliate-js/view.js");
    const CFI = await import("./vendor/foliate-js/epubcfi.js");
    const { SectionProgress } = await import("./vendor/foliate-js/progress.js");

    const port = envInfo.server_port || 18421;
    const epubs = [
      "frankenstein-pg84.epub",
      "moby-dick-pg2701.epub",
      "pride-and-prejudice-pg1342.epub",
    ];

    const container = document.getElementById("epub-container");
    const allResults = {};

    for (const epubName of epubs) {
      appendOut(`Abrindo EPUB: ${epubName}...`);
      const t0 = performance.now();
      const resp = await fetch(`http://127.0.0.1:${port}/file/${epubName}`);
      if (!resp.ok) throw new Error(`HTTP ${resp.status} ao baixar ${epubName}`);
      const blob = await resp.blob();
      const file = new File([blob], epubName, { type: "application/epub+zip" });

      container.innerHTML = "";
      const view = document.createElement("foliate-view");
      container.appendChild(view);
      await view.open(file);

      // Force identical typography in paginator for cross-platform comparison
      if (view.renderer?.setStyles) {
        view.renderer.setStyles(`
          html, body {
            font-family: serif !important;
            font-size: 16px !important;
            line-height: 1.5 !important;
            margin: 0 !important;
          }
        `);
      }

      const book = view.book;
      const sectionProgress = new SectionProgress(book.sections, 1500, 1600);
      const flatToc = flattenToc(book.toc || []);
      // Pick 5 distinct chapter entries from TOC
      const chosenToc = flatToc.filter(t => t.href).slice(0, 5);

      // 1) Deterministic DOM-level CFIs (5 chapters + 10%, 50%, 90%)
      const domChapters = [];
      for (const item of chosenToc) {
        const resolved = await book.resolveHref(item.href);
        const doc = await book.sections[resolved.index].createDocument();
        const anchorEl = resolved.anchor(doc);
        const range = doc.createRange();
        if (anchorEl && anchorEl.nodeType) {
          range.setStart(anchorEl, 0);
          range.setEnd(anchorEl, 0);
        } else {
          range.setStart(doc.body, 0);
          range.setEnd(doc.body, 0);
        }
        const cfi = view.getCFI(resolved.index, range);
        const snippet = extractSnippetFromResolvedAnchor(doc, () => range);
        domChapters.push({
          label: cleanSnippet(item.label, 50),
          href: item.href,
          section_index: resolved.index,
          cfi,
          snippet,
        });
      }

      const domFractions = [];
      for (const frac of [0.1, 0.5, 0.9]) {
        const [secIdx, fracInSec] = sectionProgress.getSection(frac);
        const doc = await book.sections[secIdx].createDocument();
        const info = rangeAtCharFraction(doc, fracInSec);
        const cfi = view.getCFI(secIdx, info.collapsedRange);
        domFractions.push({
          fraction: frac,
          section_index: secIdx,
          fraction_in_section: Number(fracInSec.toFixed(6)),
          total_chars_in_section: info.totalChars,
          target_char: info.targetChar,
          cfi,
          snippet: info.snippet,
        });
      }

      // 2) Paginator-level CFIs (navigating <foliate-view> via goTo / goToFraction)
      const paginatorChapters = [];
      for (const item of chosenToc) {
        await view.goTo(item.href);
        const loc = view.lastLocation || {};
        paginatorChapters.push({
          label: cleanSnippet(item.label, 50),
          href: item.href,
          full_range_cfi: loc.cfi,
          collapsed_start_cfi: loc.cfi ? CFI.collapse(loc.cfi) : null,
          snippet: cleanSnippet(loc.range?.toString() || "", 80),
        });
      }

      const paginatorFractions = [];
      for (const frac of [0.1, 0.5, 0.9]) {
        await view.goToFraction(frac);
        const loc = view.lastLocation || {};
        paginatorFractions.push({
          fraction: frac,
          full_range_cfi: loc.cfi,
          collapsed_start_cfi: loc.cfi ? CFI.collapse(loc.cfi) : null,
          snippet: cleanSnippet(loc.range?.toString() || "", 80),
        });
      }

      // 3) Caminho inverso: resolver CFIs de volta para documento + paginator e extrair snippet
      const toVerifyList = (foreignCfisToVerify && foreignCfisToVerify[epubName]) || [
        ...domFractions.map(f => ({ kind: `dom_${f.fraction * 100}%`, cfi: f.cfi, source_snippet: f.snippet })),
        ...domChapters.map((c, idx) => ({ kind: `dom_ch_${idx + 1}`, cfi: c.cfi, source_snippet: c.snippet })),
        ...paginatorFractions.map(f => ({ kind: `pag_${f.fraction * 100}%_collapsed`, cfi: f.collapsed_start_cfi, source_snippet: f.snippet })),
      ];

      const reverseResolutions = [];
      for (const entry of toVerifyList) {
        const resolved = view.resolveCFI(entry.cfi);
        const doc = await book.sections[resolved.index].createDocument();
        const domSnippet = extractSnippetFromResolvedAnchor(doc, resolved.anchor);
        await view.goTo(entry.cfi);
        const pagSnippet = cleanSnippet(view.lastLocation?.range?.toString() || "", 80);
        reverseResolutions.push({
          kind: entry.kind,
          cfi: entry.cfi,
          resolved_section: resolved.index,
          source_snippet: entry.source_snippet,
          resolved_dom_snippet: domSnippet,
          resolved_paginator_snippet: pagSnippet,
          exact_dom_snippet_match: domSnippet === entry.source_snippet,
          paginator_contains_prefix:
            pagSnippet.slice(0, 30) === domSnippet.slice(0, 30) ||
            pagSnippet.includes(domSnippet.slice(0, 25)) ||
            domSnippet.includes(pagSnippet.slice(0, 25)),
        });
      }

      const elapsedMs = Number((performance.now() - t0).toFixed(2));
      const epubResult = {
        epub: epubName,
        platform: envInfo.os,
        user_agent: navigator.userAgent,
        elapsed_ms: elapsedMs,
        spine_sections: book.sections.length,
        toc_entries_total: flatToc.length,
        dom_chapters: domChapters,
        dom_fractions: domFractions,
        paginator_chapters: paginatorChapters,
        paginator_fractions: paginatorFractions,
        reverse_resolutions: reverseResolutions,
      };
      allResults[epubName] = epubResult;
      await logResult(`RISCO3_EPUB_${epubName}`, epubResult);
      view.close();
    }

    await logResult("RISCO3_ALL_DONE", { platform: envInfo.os, books: Object.keys(allResults) });
    return allResults;
  } catch (err) {
    await logResult("RISCO3_ERROR", { error: String(err), stack: err?.stack });
    throw err;
  }
};

// ============================================================================
// FASE 3: Rede Local (mDNS, TLS Pinning, Pareamento, Sync HLC + SSE, Range, Doze)
// ============================================================================

const syncBannerEl = document.getElementById("sync-banner");

function loadSavedLanSession() {
  try {
    const qrRaw = localStorage.getItem("bookferry_lan_qr");
    const tok = localStorage.getItem("bookferry_lan_token");
    if (qrRaw) window.__lanQr = JSON.parse(qrRaw);
    if (tok) window.__lanToken = tok;
  } catch (_) {}
}
loadSavedLanSession();

function saveLanSession(qr, token) {
  try {
    if (qr) {
      window.__lanQr = qr;
      localStorage.setItem("bookferry_lan_qr", JSON.stringify(qr));
    }
    if (token) {
      window.__lanToken = token;
      localStorage.setItem("bookferry_lan_token", token);
    }
  } catch (_) {}
}

window.__onLanSseStatus = async (statusObj) => {
  appendOut(`[SSE_STATUS] ${JSON.stringify(statusObj)}`);
  await logResult("FASE3_SSE_STATUS_EVENT", statusObj);
};

window.__onLanSseEvent = async (evObj) => {
  const tUi0 = performance.now();
  const pos = evObj.position || {};
  const pageNum = pos.page != null ? pos.page : pos.locator;
  if (syncBannerEl) {
    syncBannerEl.textContent = `[SSE AO VIVO] Livro: ${pos.book_id} -> Página ${pageNum} (${pos.percent}%) | HLC=${pos.hlc_canonical} | dev=${pos.device}`;
    syncBannerEl.style.borderLeftColor = "#a6e3a1";
    syncBannerEl.style.color = "#a6e3a1";
  }

  await new Promise((r) => requestAnimationFrame(() => r()));
  const uiRenderDeltaMs = Number((performance.now() - tUi0).toFixed(3));
  const nowUnixMs = Date.now();

  let ackRes = null;
  if (window.__lanQr && window.__lanToken) {
    try {
      ackRes = await invoke("lan_ack_ui_rendered", {
        host: window.__lanQr.ip,
        port: window.__lanQr.port,
        certSha256: window.__lanQr.cert_sha256,
        token: window.__lanToken,
        bookId: pos.book_id,
        page: pos.page ?? null,
        hlc: pos.hlc_canonical,
        serverReceivedUnixMs: pos.server_received_unix_ms,
        uiRenderDeltaMs,
      });
    } catch (e) {
      console.error("ack err:", e);
    }
  }

  const report = {
    source: "SSE_PUSH",
    book_id: pos.book_id,
    page: pos.page,
    locator: pos.locator,
    percent: pos.percent,
    device: pos.device,
    hlc_canonical: pos.hlc_canonical,
    server_seq: pos.server_seq,
    server_received_unix_ms: pos.server_received_unix_ms,
    android_rust_recv_unix_ms: evObj.android_rust_recv_unix_ms,
    android_ui_rendered_unix_ms: nowUnixMs,
    rust_to_ui_dom_ms: uiRenderDeltaMs,
    banner_text: syncBannerEl ? syncBannerEl.textContent : "",
    server_ack_roundtrip: ackRes,
  };
  await logResult("FASE3_SSE_UI_UPDATED", report);
};

window.runFase3MdnsColdOne = async (rep = 1, timeoutMs = 2200) => {
  const mcastHeld = window.AndroidSaf && window.AndroidSaf.setMulticastLock
    ? window.AndroidSaf.setMulticastLock(true)
    : false;
  appendOut(`Rodando mDNS Cold rep=${rep} (multicastLock=${mcastHeld})...`);
  const rustRes = await invoke("lan_discover_mdns", {
    rep,
    mode: "cold",
    timeoutMs,
  });
  let androidNsd = null;
  if (!rustRes.ok && window.AndroidSaf && window.AndroidSaf.discoverNsdAndroid) {
    try {
      androidNsd = JSON.parse(window.AndroidSaf.discoverNsdAndroid(1500));
    } catch (e) {
      androidNsd = { error: String(e) };
    }
  }
  const out = {
    rep,
    mode: "cold",
    multicast_lock_held: mcastHeld,
    rust_mdns_sd: rustRes,
    android_nsd_manager: androidNsd,
  };
  await logResult("FASE3_MDNS_COLD", out);
  return out;
};

window.runFase3MdnsWarmSuite = async (reps = 5, timeoutMs = 2200) => {
  const mcastHeld = window.AndroidSaf && window.AndroidSaf.setMulticastLock
    ? window.AndroidSaf.setMulticastLock(true)
    : false;
  appendOut(`Rodando suíte mDNS Warm (${reps} repetições, multicastLock=${mcastHeld})...`);
  const runs = [];
  for (let r = 1; r <= reps; r++) {
    const res = await invoke("lan_discover_mdns", {
      rep: r,
      mode: "warm",
      timeoutMs,
    });
    runs.push(res);
  }
  let androidNsd = null;
  if (window.AndroidSaf && window.AndroidSaf.discoverNsdAndroid) {
    try {
      androidNsd = JSON.parse(window.AndroidSaf.discoverNsdAndroid(1800));
    } catch (e) {
      androidNsd = { error: String(e) };
    }
  }
  const out = {
    mode: "warm",
    multicast_lock_held: mcastHeld,
    runs,
    android_nsd_manager: androidNsd,
  };
  await logResult("FASE3_MDNS_WARM", out);
  return out;
};

window.runFase3TlsAndPairing = async (host = "192.168.1.2", port = 47600) => {
  try {
    appendOut(`Lendo QR simulado do servidor em ${host}:${port}...`);
    const qr = await invoke("lan_read_simulated_qr", { host, port });
    saveLanSession(qr, null);
    await logResult("FASE3_QR_SIMULATED", qr);

    appendOut("Executando testes de TLS com Pinning (5 reps hash correto, hash errado, HTTP puro)...");
    const tlsReport = await invoke("lan_test_tls_pinning", {
      host: qr.ip,
      port: qr.port,
      correctSha256: qr.cert_sha256,
      repetitions: 5,
    });
    await logResult("FASE3_TLS_PINNING", tlsReport);

    appendOut("Executando testes de Pareamento (token inválido=401, segredo errado=401, segredo uso único=200, reuso=403)...");
    const pairReport = await invoke("lan_test_pairing", {
      host: qr.ip,
      port: qr.port,
      certSha256: qr.cert_sha256,
      oneTimeSecret: qr.pairing_secret,
    });
    if (pairReport.issued_token) {
      saveLanSession(qr, pairReport.issued_token);
    }
    await logResult("FASE3_PAIRING", pairReport);

    if (window.__lanToken) {
      appendOut("Iniciando listener SSE sobre TLS com Pinning...");
      const sseStart = await invoke("lan_start_sse_listener", {
        host: qr.ip,
        port: qr.port,
        certSha256: qr.cert_sha256,
        token: window.__lanToken,
      });
      await logResult("FASE3_SSE_STARTED", sseStart);
    }

    return { qr, tlsReport, pairReport };
  } catch (err) {
    await logResult("FASE3_TLS_PAIR_ERROR", { error: String(err) });
    throw err;
  }
};

window.runFase3EnsureSse = async () => {
  loadSavedLanSession();
  if (!window.__lanQr || !window.__lanToken) {
    throw new Error("Sessao LAN nao encontrada; rode runFase3TlsAndPairing primeiro");
  }
  const res = await invoke("lan_start_sse_listener", {
    host: window.__lanQr.ip,
    port: window.__lanQr.port,
    certSha256: window.__lanQr.cert_sha256,
    token: window.__lanToken,
  });
  await logResult("FASE3_ENSURE_SSE", res);
  return res;
};

window.runFase3PullPosition = async (bookId = "little-nemo") => {
  loadSavedLanSession();
  const t0 = performance.now();
  const res = await invoke("lan_get_position", {
    host: window.__lanQr.ip,
    port: window.__lanQr.port,
    certSha256: window.__lanQr.cert_sha256,
    token: window.__lanToken,
    bookId,
  });
  const pos = res.response?.position || {};
  const pageNum = pos.page != null ? pos.page : pos.locator;
  if (syncBannerEl && pos.book_id) {
    syncBannerEl.textContent = `[PULL AO REABRIR] Livro: ${pos.book_id} -> Página ${pageNum} (${pos.percent}%) | HLC=${pos.hlc_canonical} | dev=${pos.device}`;
    syncBannerEl.style.borderLeftColor = "#f9e2af";
    syncBannerEl.style.color = "#f9e2af";
  }
  await new Promise((r) => requestAnimationFrame(() => r()));
  const totalUiPullMs = Number((performance.now() - t0).toFixed(3));
  const out = {
    source: "PULL_ON_REOPEN",
    total_ui_pull_ms: totalUiPullMs,
    banner_text: syncBannerEl ? syncBannerEl.textContent : "",
    ...res,
  };
  await logResult("FASE3_PULL_POSITION", out);
  return out;
};

window.runFase3LwwConflictTest = async (bookId = "little-nemo") => {
  loadSavedLanSession();
  const host = window.__lanQr.ip;
  const port = window.__lanQr.port;
  const certSha256 = window.__lanQr.cert_sha256;
  const token = window.__lanToken;

  // Base HLC no futuro próximo para isolar o cenário concorrente de testes anteriores
  const baseWall = Date.now() + 10000;

  // 1) PC escreve #1 (pág. 60, wall = baseWall + 100)
  const w1 = await invoke("lan_put_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
    device: "pc-linux",
    hlc: `${baseWall + 100}:000000:pc-linux`,
    locator: "page:60",
    percent: 14.25,
    page: 60,
  });

  // 2) Celular escreve concorrente (pág. 65, wall = baseWall + 200 -> ganha de PC #1)
  const w2 = await invoke("lan_put_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
    device: "android-s23",
    hlc: `${baseWall + 200}:000000:android-s23`,
    locator: "page:65",
    percent: 15.44,
    page: 65,
  });

  // 3) PC escreve #2 (pág. 72, wall = baseWall + 300 -> ganha do celular)
  const w3 = await invoke("lan_put_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
    device: "pc-linux",
    hlc: `${baseWall + 300}:000000:pc-linux`,
    locator: "page:72",
    percent: 17.1,
    page: 72,
  });

  // 4) Escrita concorrente atrasada do celular (pág. 66, wall = baseWall + 250 < baseWall + 300 -> perde no LWW mas fica salva no histórico!)
  const w4_stale = await invoke("lan_put_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
    device: "android-s23",
    hlc: `${baseWall + 250}:000001:android-s23`,
    locator: "page:66",
    percent: 15.68,
    page: 66,
  });

  // 5) Empate de wall_ms, desempate por contador lógico HLC (wall = baseWall + 300, logical = 1 -> ganha de logical = 0!)
  const w5_logical_tiebreak = await invoke("lan_put_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
    device: "android-s23",
    hlc: `${baseWall + 300}:000001:android-s23`,
    locator: "page:75",
    percent: 17.81,
    page: 75,
  });

  const finalGet = await invoke("lan_get_position", {
    host,
    port,
    certSha256,
    token,
    bookId,
  });

  const out = {
    book_id: bookId,
    step1_pc_write_1: w1.response,
    step2_android_concurrent_newer: w2.response,
    step3_pc_write_2_newer: w3.response,
    step4_android_stale_concurrent_rejected_without_data_loss: w4_stale.response,
    step5_hlc_logical_counter_tiebreak: w5_logical_tiebreak.response,
    final_state_and_audit_history: finalGet.response,
  };
  await logResult("FASE3_LWW_CONFLICT", out);
  return out;
};

window.runFase3RangeTransfer = async (fileName = "little-nemo-1905-1914.cbz") => {
  loadSavedLanSession();
  appendOut(`Iniciando transferência HTTP Range sobre TLS de ${fileName} (295 MB)...`);
  const targetOutName = "lan-downloaded-little-nemo.cbz";
  let fdInfo = null;
  if (window.AndroidSaf && window.AndroidSaf.createWritableDownloadFd) {
    fdInfo = JSON.parse(window.AndroidSaf.createWritableDownloadFd(targetOutName));
  } else {
    throw new Error("AndroidSaf.createWritableDownloadFd indisponivel");
  }
  if (!fdInfo.ok) {
    throw new Error("Falha ao criar arquivo de destino: " + JSON.stringify(fdInfo));
  }

  // Interrompe exatamente na metade (~147.728.971 bytes de 295.457.942 bytes) e retoma com Range!
  const interruptAt = 147728971;
  const rep = await invoke("lan_download_cbz_range", {
    host: window.__lanQr.ip,
    port: window.__lanQr.port,
    certSha256: window.__lanQr.cert_sha256,
    token: window.__lanToken,
    fileName,
    fd: fdInfo.fd,
    destPath: fdInfo.path,
    interruptAfterBytes: interruptAt,
  });

  const fullOut = {
    fd_creation_method: fdInfo.method,
    ...rep,
  };
  await logResult("FASE3_RANGE_TRANSFER", fullOut);
  return fullOut;
};

window.runFase3DumpSseState = async (label = "snapshot") => {
  const st = await invoke("lan_get_sse_state");
  await logResult(`FASE3_SSE_STATE_${label}`, st);
  return st;
};

// ============================================================================
// UI Bindings & Autorun
// ============================================================================

document.getElementById("btn-ping").addEventListener("click", async () => {
  const res = await invoke("ping", { name: "Bookferry" });
  appendOut(res);
});

document.getElementById("btn-f3-mdns")?.addEventListener("click", () => {
  window.runFase3MdnsWarmSuite(5);
});

document.getElementById("btn-f3-pair")?.addEventListener("click", () => {
  window.runFase3TlsAndPairing();
});

document.getElementById("btn-f3-pull")?.addEventListener("click", () => {
  window.runFase3PullPosition();
});

document.getElementById("btn-f3-lww")?.addEventListener("click", () => {
  window.runFase3LwwConflictTest();
});

document.getElementById("btn-f3-range")?.addEventListener("click", () => {
  window.runFase3RangeTransfer();
});

document.getElementById("btn-saf-one")?.addEventListener("click", () => {
  if (window.AndroidSaf) window.AndroidSaf.pickDocument(false);
  else appendOut("AndroidSaf nao disponivel neste SO");
});

document.getElementById("btn-saf-multi")?.addEventListener("click", () => {
  if (window.AndroidSaf) window.AndroidSaf.pickDocument(true);
  else appendOut("AndroidSaf nao disponivel neste SO");
});

document.getElementById("btn-r2-cbz")?.addEventListener("click", () => {
  window.runRisco2RafBench();
});

document.getElementById("btn-r2-pdf")?.addEventListener("click", () => {
  window.runRisco2PdfBench();
});

document.getElementById("btn-r3-epub")?.addEventListener("click", () => {
  window.runRisco3EpubBench();
});

(async function init() {
  try {
    envInfo = await invoke("get_env_info");
    appendOut(`Inicializado: os=${envInfo.os} arch=${envInfo.arch} port=${envInfo.server_port} autorun=${envInfo.autorun || "none"}`);
    if (envInfo.autorun === "risco3") {
      await window.runRisco3EpubBench();
      await invoke("log_result", { tag: "AUTORUN_DONE", payload: "{\"done\":true}" });
    }
  } catch (e) {
    appendOut("Erro init: " + e);
  }
})();
