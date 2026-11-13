// Interface do leitor com DOM: tema do "chrome", painel "Aa" (EPUB) e barra de progresso.
// A lógica pura (validação, CSS, formatação) fica em state.js e é coberta por `node --test`.

import { api } from './api.js';
import {
  BUILTIN_PRESETS,
  EPUB_STYLE_FIELDS,
  applyPresetToPrefs,
  formatProgressLine,
  nextPagedFormat,
  sanitizeProgressPrefs,
  sanitizeReaderPrefs,
  validatePresetName,
} from './state.js';

const $ = (id) => document.getElementById(id);

/** Aplica o tema efetivo do livro ao "chrome" do leitor (toolbar, footer, fundo). */
export function setReaderChromeTheme(effectiveTheme) {
  const screen = $('reader-screen');
  if (!screen) return;
  if (effectiveTheme) screen.dataset.readerTheme = effectiveTheme;
  else delete screen.dataset.readerTheme;
}

function errMessage(err) {
  return String(err?.message || err || 'Erro desconhecido');
}

function samePresetStyle(a, b) {
  const x = sanitizeReaderPrefs(a);
  const y = sanitizeReaderPrefs(b);
  return EPUB_STYLE_FIELDS.every((f) => x[f] === y[f]);
}

/**
 * Painel de aparência do EPUB. Cada alteração é aplicada ao vivo (`reader.applyPrefs`) e salva
 * por livro (com debounce); presets, padrão global e itens do rodapé são gravados no Rust.
 * @param {{ bookId: string, view: object, reader: { applyPrefs: Function }, onProgressPrefs: (p: object) => void }} opts
 */
export function mountAaPanel({ bookId, view: initialView, reader, onProgressPrefs }) {
  const panel = $('aa-panel');
  const toggleBtn = $('btn-reader-aa');
  if (!panel || !toggleBtn) return { destroy() {}, close() {}, isOpen: () => false };

  let view = initialView;
  let prefs = sanitizeReaderPrefs(view.prefs);
  let saveTimer = null;
  let saveChain = Promise.resolve();
  let statusTimer = null;
  let destroyed = false;
  const cleanups = [];

  const on = (target, type, handler, opts) => {
    target.addEventListener(type, handler, opts);
    cleanups.push(() => target.removeEventListener(type, handler, opts));
  };

  const el = {
    size: $('aa-font-size'),
    sizeOut: $('aa-font-size-out'),
    family: $('aa-font-family'),
    lh: $('aa-line-height'),
    lhOut: $('aa-line-height-out'),
    margin: $('aa-margin'),
    marginOut: $('aa-margin-out'),
    align: $('aa-align'),
    hyph: $('aa-hyphenate'),
    presets: $('aa-presets'),
    presetOpen: $('aa-preset-save-open'),
    presetForm: $('aa-preset-form'),
    presetName: $('aa-preset-name'),
    presetError: $('aa-preset-error'),
    status: $('aa-status'),
  };
  const themeBtns = [...panel.querySelectorAll('.aa-theme')];
  const progressChecks = [...panel.querySelectorAll('input[data-progress]')];

  function say(msg) {
    if (!el.status) return;
    el.status.textContent = msg;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      if (el.status) el.status.textContent = '';
    }, 2500);
  }

  function showPresetError(msg) {
    if (!el.presetError) return;
    el.presetError.textContent = msg || '';
    el.presetError.classList.toggle('hidden', !msg);
  }

  function renderPresets() {
    if (!el.presets) return;
    el.presets.innerHTML = '';
    const all = [
      ...BUILTIN_PRESETS.map((p) => ({ ...p, builtin: true })),
      ...(Array.isArray(view.presets) ? view.presets : []).map((p) => ({ ...p, builtin: false })),
    ];
    for (const p of all) {
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'aa-chip';
      chip.textContent = p.name;
      chip.dataset.preset = p.name;
      chip.setAttribute('aria-pressed', String(samePresetStyle(prefs, p.prefs)));
      chip.addEventListener('click', () => {
        setPrefs(applyPresetToPrefs(prefs, p.prefs));
      });
      if (p.builtin) {
        el.presets.appendChild(chip);
        continue;
      }
      const wrap = document.createElement('span');
      wrap.className = 'aa-chip-wrap';
      const del = document.createElement('button');
      del.type = 'button';
      del.className = 'aa-chip-del';
      del.textContent = '✕';
      del.setAttribute('aria-label', `Apagar predefinição ${p.name}`);
      del.addEventListener('click', async () => {
        try {
          view = await api.deleteReaderPreset(bookId, p.name);
          renderPresets();
          say('Predefinição apagada.');
        } catch (err) {
          say(errMessage(err));
        }
      });
      wrap.append(chip, del);
      el.presets.appendChild(wrap);
    }
  }

  function renderControls() {
    el.size.value = String(prefs.font_size);
    el.sizeOut.textContent = `${prefs.font_size}px`;
    el.family.value = prefs.font_family;
    el.lh.value = String(prefs.line_height);
    el.lhOut.textContent = prefs.line_height.toFixed(2);
    el.margin.value = String(prefs.margin_pct);
    el.marginOut.textContent = `${prefs.margin_pct}%`;
    el.align.value = prefs.align;
    el.hyph.checked = prefs.hyphenate;
    for (const b of themeBtns) {
      b.setAttribute('aria-checked', String(b.dataset.theme === prefs.theme));
      b.tabIndex = b.dataset.theme === prefs.theme ? 0 : -1;
    }
    const progress = sanitizeProgressPrefs(view.progress);
    for (const c of progressChecks) c.checked = Boolean(progress[c.dataset.progress]);
    for (const chip of el.presets?.querySelectorAll('.aa-chip') || []) {
      const all = [...BUILTIN_PRESETS, ...(view.presets || [])];
      const p = all.find((x) => x.name === chip.dataset.preset);
      if (p) chip.setAttribute('aria-pressed', String(samePresetStyle(prefs, p.prefs)));
    }
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(flushSave, 400);
  }

  function flushSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = null;
    const snapshot = prefs;
    saveChain = saveChain
      .then(() => api.saveReaderPrefs(bookId, snapshot))
      .then((v) => {
        // Só `is_custom` vem da resposta: presets/padrões têm as próprias operações.
        view = { ...view, is_custom: v.is_custom };
      })
      .catch((err) => say(errMessage(err)));
    return saveChain;
  }

  /** Aplica (ao vivo) e agenda o salvamento por livro. */
  function setPrefs(next) {
    prefs = sanitizeReaderPrefs(next);
    renderControls();
    reader.applyPrefs(prefs);
    scheduleSave();
  }

  // ----- Controles -----
  on(el.size, 'input', () => setPrefs({ ...prefs, font_size: Number(el.size.value) }));
  on(el.family, 'change', () => setPrefs({ ...prefs, font_family: el.family.value }));
  on(el.lh, 'input', () => setPrefs({ ...prefs, line_height: Number(el.lh.value) }));
  on(el.margin, 'input', () => setPrefs({ ...prefs, margin_pct: Number(el.margin.value) }));
  on(el.align, 'change', () => setPrefs({ ...prefs, align: el.align.value }));
  on(el.hyph, 'change', () => setPrefs({ ...prefs, hyphenate: el.hyph.checked }));
  for (const b of themeBtns) {
    on(b, 'click', () => setPrefs({ ...prefs, theme: b.dataset.theme }));
    on(b, 'keydown', (ev) => {
      const i = themeBtns.indexOf(b);
      let j = -1;
      if (ev.key === 'ArrowRight' || ev.key === 'ArrowDown') j = (i + 1) % themeBtns.length;
      if (ev.key === 'ArrowLeft' || ev.key === 'ArrowUp') {
        j = (i - 1 + themeBtns.length) % themeBtns.length;
      }
      if (j >= 0) {
        ev.preventDefault();
        ev.stopPropagation();
        themeBtns[j].focus();
        themeBtns[j].click();
      }
    });
  }
  for (const c of progressChecks) {
    on(c, 'change', async () => {
      const progress = sanitizeProgressPrefs({
        ...sanitizeProgressPrefs(view.progress),
        [c.dataset.progress]: c.checked,
      });
      try {
        view = await api.saveProgressPrefs(bookId, progress);
        onProgressPrefs?.(sanitizeProgressPrefs(view.progress));
      } catch (err) {
        say(errMessage(err));
      }
    });
  }

  // ----- Predefinições -----
  on(el.presetOpen, 'click', () => {
    showPresetError('');
    el.presetForm.classList.toggle('hidden');
    if (!el.presetForm.classList.contains('hidden')) el.presetName.focus();
  });
  on(el.presetForm, 'submit', async (ev) => {
    ev.preventDefault();
    const checked = validatePresetName(el.presetName.value);
    if (!checked.ok) {
      showPresetError(checked.error);
      return;
    }
    try {
      view = await api.saveReaderPreset(bookId, checked.name, prefs);
      el.presetName.value = '';
      el.presetForm.classList.add('hidden');
      showPresetError('');
      renderPresets();
      renderControls();
      say(`Predefinição "${checked.name}" salva.`);
    } catch (err) {
      showPresetError(errMessage(err));
    }
  });

  on($('aa-set-default'), 'click', async () => {
    try {
      await flushSave();
      view = await api.setDefaultReaderPrefs(bookId, prefs);
      say('Padrão para novos livros atualizado.');
    } catch (err) {
      say(errMessage(err));
    }
  });
  on($('aa-reset'), 'click', async () => {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = null;
    try {
      await saveChain;
      view = await api.resetReaderPrefs(bookId);
      prefs = sanitizeReaderPrefs(view.prefs);
      renderPresets();
      renderControls();
      reader.applyPrefs(prefs);
      say('Livro voltou ao padrão.');
    } catch (err) {
      say(errMessage(err));
    }
  });

  // ----- Abrir / fechar -----
  function isOpen() {
    return !panel.classList.contains('hidden');
  }
  function open() {
    panel.classList.remove('hidden');
    toggleBtn.setAttribute('aria-expanded', 'true');
    (themeBtns.find((b) => b.tabIndex === 0) || el.size)?.focus?.({ preventScroll: true });
  }
  function close({ restoreFocus = true } = {}) {
    if (!isOpen()) return;
    panel.classList.add('hidden');
    toggleBtn.setAttribute('aria-expanded', 'false');
    if (restoreFocus) toggleBtn.focus({ preventScroll: true });
  }
  on(toggleBtn, 'click', () => (isOpen() ? close() : open()));
  on(document, 'pointerdown', (ev) => {
    if (!isOpen()) return;
    if (panel.contains(ev.target) || toggleBtn.contains(ev.target)) return;
    close({ restoreFocus: false });
  });
  on(panel, 'keydown', (ev) => {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      ev.stopPropagation();
      close();
    }
  });

  showPresetError('');
  el.presetForm.classList.add('hidden');
  renderPresets();
  renderControls();

  return {
    isOpen,
    open,
    close,
    getPrefs: () => ({ ...prefs }),
    /** Chama ao fechar o leitor: grava o que estiver pendente. */
    async flush() {
      if (saveTimer) await flushSave();
      else await saveChain;
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (statusTimer) clearTimeout(statusTimer);
      close({ restoreFocus: false });
      for (const fn of cleanups.splice(0)) fn();
    },
  };
}

/**
 * Barra de progresso + rótulo do rodapé. `update(info)` recebe o que o leitor sabe
 * (`kind: 'epub' | 'paged'`); clicar/arrastar a barra faz seek; clicar no rótulo de PDF/CBZ
 * cicla o formato do texto.
 * @param {{ getReader: () => object | null, bookId: () => string | null }} deps
 */
export function createProgressBar({ getReader, bookId }) {
  const bar = $('reader-progress');
  const fill = $('reader-progress-fill');
  const label = $('reader-status-label');
  let items = sanitizeProgressPrefs(null);
  let lastInfo = null;
  let seeking = false;

  function fractionOf(info) {
    if (!info) return 0;
    if (info.kind === 'paged') {
      const total = Number(info.totalPages) || 1;
      const last = Math.max(Number(info.pageEnd) || 0, Number(info.page) || 1);
      return Math.max(0, Math.min(1, total <= 1 ? 1 : (last - 1) / (total - 1)));
    }
    return Math.max(0, Math.min(1, Number(info.bookFraction) || 0));
  }

  function render() {
    if (!label || !fill || !bar) return;
    const frac = fractionOf(lastInfo);
    fill.style.width = `${(frac * 100).toFixed(2)}%`;
    bar.setAttribute('aria-valuenow', String(Math.round(frac * 100)));
    let text = formatProgressLine(lastInfo, items, items.page_format);
    if (lastInfo?.kind === 'paged' && lastInfo.zoomPct && lastInfo.zoomPct !== 100) {
      text += ` · ${lastInfo.zoomPct}%`;
    }
    label.textContent = text || '—';
    label.title =
      lastInfo?.kind === 'paged' ? 'Clique para alternar o formato' : label.textContent;
  }

  function fractionFromPointer(ev) {
    const rect = bar.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    return Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
  }

  async function seek(fraction) {
    const reader = getReader();
    if (!reader) return;
    if (reader.format === 'epub') {
      await reader.seekFraction?.(fraction);
    } else {
      const total = Number(lastInfo?.totalPages) || 1;
      await reader.seekPage?.(Math.round(fraction * (total - 1)) + 1);
    }
  }

  function previewFraction(f) {
    fill.style.width = `${(f * 100).toFixed(2)}%`;
  }

  bar?.addEventListener('pointerdown', (ev) => {
    seeking = true;
    try {
      bar.setPointerCapture?.(ev.pointerId);
    } catch (_) {}
    previewFraction(fractionFromPointer(ev));
  });
  bar?.addEventListener('pointermove', (ev) => {
    if (seeking) previewFraction(fractionFromPointer(ev));
  });
  bar?.addEventListener('pointerup', async (ev) => {
    if (!seeking) return;
    seeking = false;
    try {
      await seek(fractionFromPointer(ev));
    } catch (_) {
      render();
    }
  });
  bar?.addEventListener('pointercancel', () => {
    seeking = false;
    render();
  });
  bar?.addEventListener('keydown', async (ev) => {
    const reader = getReader();
    if (!reader) return;
    if (ev.key === 'ArrowRight' || ev.key === 'ArrowUp') {
      ev.preventDefault();
      ev.stopPropagation();
      await reader.next?.();
    } else if (ev.key === 'ArrowLeft' || ev.key === 'ArrowDown') {
      ev.preventDefault();
      ev.stopPropagation();
      await reader.prev?.();
    }
  });

  label?.addEventListener('click', async () => {
    if (lastInfo?.kind !== 'paged') return;
    items = sanitizeProgressPrefs({ ...items, page_format: nextPagedFormat(items.page_format) });
    render();
    const id = bookId();
    if (id) {
      try {
        await api.saveProgressPrefs(id, items);
      } catch (_) {}
    }
  });

  return {
    update(info) {
      lastInfo = info;
      if (!seeking) render();
    },
    setItems(next) {
      items = sanitizeProgressPrefs(next);
      render();
    },
    getItems: () => ({ ...items }),
    reset() {
      lastInfo = null;
      render();
    },
  };
}
