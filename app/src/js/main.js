// Ponto de entrada do frontend Bookferry (Tauri v2 + ES Modules).

import { api, listenClientEvents, setProtocolBaseUrl } from './api.js';
import {
  prepareBookToOpen,
  renderLibraryView,
  updateSyncIndicator,
} from './library.js';
import { mountCbzReader } from './readers/cbz.js';
import { mountEpubReader } from './readers/epub.js';
import { mountPdfReader } from './readers/pdf.js';
import { createProgressBar, mountAaPanel, setReaderChromeTheme } from './reader-ui.js';
import { formatBytes, formatPercent, sanitizeProgressPrefs } from './state.js';
import {
  applyTheme,
  downloadModal,
  hideToast,
  refreshPairedDevicesList,
  registerKnownDeviceName,
  renderDesktopServerPanel,
  resolveDeviceLabel,
  showToast,
  updatePairingQrDisplay,
} from './ui.js';

const appState = {
  status: null,
  books: [],
  isStale: false,
  searchQuery: '',
  activeBook: null,
  activeReader: null,
  aaPanel: null,
  progressBar: null,
};

function showScreen(screenId) {
  const screens = ['pairing-screen', 'library-screen', 'reader-screen'];
  for (const id of screens) {
    const el = document.getElementById(id);
    if (el) {
      el.classList.toggle('hidden', id !== screenId);
    }
  }
}

async function refreshStatus() {
  const st = await api.status();
  appState.status = st;
  setProtocolBaseUrl(st.protocol_base_url);
  applyTheme(st.settings?.theme || 'auto');

  const isDesktop = st.platform === 'desktop';

  const versionEl = document.getElementById('settings-app-version');
  if (versionEl) versionEl.textContent = `v${st.version}`;

  const deviceNameInput = document.getElementById('settings-device-name');
  if (deviceNameInput && !deviceNameInput.value) {
    deviceNameInput.value = isDesktop ? 'Bookferry Desktop' : 'Bookferry Android';
  }

  const fontSizeSelect = document.getElementById('settings-font-size');
  if (fontSizeSelect && st.settings?.default_font_size) {
    fontSizeSelect.value = String(st.settings.default_font_size);
  }

  const themeSelect = document.getElementById('settings-theme-select');
  if (themeSelect && st.settings?.theme) {
    themeSelect.value = st.settings.theme;
  }

  const desktopPairVaultBlock = document.getElementById('pairing-desktop-vault-block');
  const settingsDesktopServer = document.getElementById('settings-desktop-server');
  if (isDesktop) {
    desktopPairVaultBlock?.classList.remove('hidden');
    settingsDesktopServer?.classList.remove('hidden');
  } else {
    desktopPairVaultBlock?.classList.add('hidden');
    settingsDesktopServer?.classList.add('hidden');
  }

  const fpEl = document.getElementById('settings-server-fp');
  if (fpEl) {
    const fp = st.paired_server?.cert_sha256;
    fpEl.textContent = fp ? `${fp.slice(0, 24)}…` : 'Não pareado';
  }

  if (isDesktop) {
    await renderDesktopServerPanel(st.embedded_server);
  }

  return st;
}

export async function refreshLibrary(showSyncSpinner = true) {
  if (showSyncSpinner) {
    updateSyncIndicator('syncing', appState.isStale);
  }
  try {
    const view = await api.library();
    appState.books = Array.isArray(view?.books) ? view.books : [];
    appState.isStale = Boolean(view?.stale);
    updateSyncIndicator(view?.stale ? 'offline' : 'connected', appState.isStale);
    renderLibraryView(
      appState.books,
      appState.searchQuery,
      (book) => openBookFlow(book),
      (book) => removeBookDownloadFlow(book),
    );
  } catch (err) {
    updateSyncIndicator('offline', true);
    showToast(`Erro ao carregar biblioteca: ${err}`);
  }
}

async function removeBookDownloadFlow(book) {
  try {
    await api.removeDownload(book.book_id);
    showToast(`Cópia local de "${book.title}" removida.`);
    await refreshLibrary(false);
  } catch (err) {
    showToast(`Erro ao remover download: ${err}`);
  }
}

async function saveReaderPosition(bookId, locator, percent, snippet) {
  try {
    const pos = await api.setPosition(bookId, locator, percent, snippet);
    const idx = appState.books.findIndex((b) => b.book_id === bookId);
    if (idx >= 0) {
      appState.books[idx] = { ...appState.books[idx], position: pos };
    }
  } catch (_) {
    // Em modo offline o set_position já grava no SQLite local com dirty=1
  }
}

function updateReaderToolbarForFormat(format) {
  const isEpub = format === 'epub';
  document.getElementById('reader-epub-controls')?.classList.toggle('hidden', !isEpub);
  document.getElementById('reader-zoom-controls')?.classList.toggle('hidden', isEpub);
  document.getElementById('btn-reader-toc')?.classList.toggle('hidden', !isEpub);
}

export async function openBookFlow(book) {
  const prepared = await prepareBookToOpen(book);
  if (!prepared) return;

  const { book: readyBook, initialPosition } = prepared;
  appState.activeBook = readyBook;

  // Atualiza status de baixado na lista local
  const idx = appState.books.findIndex((b) => b.book_id === readyBook.book_id);
  if (idx >= 0) {
    appState.books[idx] = { ...appState.books[idx], downloaded: true };
  }

  const titleEl = document.getElementById('reader-book-title');
  const statusLabelEl = document.getElementById('reader-status-label');
  const viewportEl = document.getElementById('reader-viewport');
  const tocListEl = document.getElementById('toc-list');

  if (titleEl) titleEl.textContent = readyBook.title;
  if (statusLabelEl) statusLabelEl.textContent = 'Abrindo...';
  document.getElementById('toc-drawer')?.classList.add('hidden');

  updateReaderToolbarForFormat(readyBook.format);
  showScreen('reader-screen');

  try {
    await teardownReader();

    const fmt = String(readyBook.format).toLowerCase();
    const bar = appState.progressBar;
    bar?.reset();
    // Preferências de leitura (por livro / padrão / presets / itens do rodapé), validadas no Rust.
    let prefsView = null;
    try {
      prefsView = await api.getReaderPrefs(readyBook.book_id);
    } catch (_) {}
    bar?.setItems(sanitizeProgressPrefs(prefsView?.progress));
    const hooks = { onProgress: (info) => bar?.update(info) };

    if (fmt === 'epub') {
      const reader = await mountEpubReader({
        book: readyBook,
        initialPosition,
        prefs: prefsView?.prefs,
        appTheme: appState.status?.settings?.theme || 'auto',
        viewportEl,
        tocListEl,
        onSavePosition: saveReaderPosition,
        hooks: { ...hooks, onThemeChange: setReaderChromeTheme },
      });
      appState.activeReader = reader;
      if (reader && prefsView) {
        appState.aaPanel = mountAaPanel({
          bookId: readyBook.book_id,
          view: prefsView,
          reader,
          onProgressPrefs: (p) => bar?.setItems(p),
        });
      }
    } else if (fmt === 'pdf') {
      appState.activeReader = await mountPdfReader({
        book: readyBook,
        initialPosition,
        viewportEl,
        statusLabelEl,
        onSavePosition: saveReaderPosition,
        hooks,
      });
    } else if (fmt === 'cbz') {
      appState.activeReader = await mountCbzReader({
        book: readyBook,
        initialPosition,
        viewportEl,
        statusLabelEl,
        onSavePosition: saveReaderPosition,
        hooks,
      });
    } else {
      throw new Error(`Formato não suportado: ${readyBook.format}`);
    }
  } catch (err) {
    showToast(`Erro ao abrir leitor: ${err}`);
    await closeActiveReader();
  }
}

async function teardownReader() {
  if (appState.aaPanel) {
    try {
      await appState.aaPanel.flush();
    } catch (_) {}
    appState.aaPanel.destroy();
    appState.aaPanel = null;
  }
  if (appState.activeReader) {
    try {
      await appState.activeReader.destroy();
    } catch (_) {}
    appState.activeReader = null;
  }
  setReaderChromeTheme(null);
}

export async function closeActiveReader() {
  await teardownReader();
  appState.progressBar?.reset();
  appState.activeBook = null;
  document.getElementById('toc-drawer')?.classList.add('hidden');
  showScreen('library-screen');
  await refreshLibrary(false);
}

function setupEventListeners() {
  // Busca por título
  const searchInput = document.getElementById('library-search-input');
  searchInput?.addEventListener('input', (ev) => {
    appState.searchQuery = ev.target.value || '';
    renderLibraryView(
      appState.books,
      appState.searchQuery,
      (book) => openBookFlow(book),
      (book) => removeBookDownloadFlow(book),
    );
  });

  // Botão Atualizar (e reescanear no desktop se servidor local estiver ativo)
  const refreshBtn = document.getElementById('btn-refresh-library');
  refreshBtn?.addEventListener('click', async () => {
    updateSyncIndicator('syncing', appState.isStale);
    if (appState.status?.platform === 'desktop' && appState.status?.embedded_server?.running) {
      try {
        await api.rescan();
      } catch (_) {}
    }
    await refreshLibrary(true);
  });

  // Pull-to-refresh por toque no topo da área de rolagem da biblioteca
  const scrollArea = document.getElementById('library-scroll-area');
  let touchStartY = null;
  scrollArea?.addEventListener(
    'touchstart',
    (ev) => {
      if (scrollArea.scrollTop <= 2 && ev.touches.length === 1) {
        touchStartY = ev.touches[0].clientY;
      } else {
        touchStartY = null;
      }
    },
    { passive: true },
  );
  scrollArea?.addEventListener(
    'touchend',
    async (ev) => {
      if (touchStartY !== null && ev.changedTouches.length === 1) {
        const dy = ev.changedTouches[0].clientY - touchStartY;
        touchStartY = null;
        if (dy > 75 && scrollArea.scrollTop <= 2) {
          await refreshLibrary(true);
        }
      }
    },
    { passive: true },
  );

  // Tela de pareamento: colar JSON do QR
  const pairBtn = document.getElementById('btn-submit-pair');
  pairBtn?.addEventListener('click', async () => {
    const input = document.getElementById('pairing-qr-input');
    const qrJson = input?.value?.trim() || '';
    if (!qrJson) {
      showToast('Cole o JSON do QR de pareamento.');
      return;
    }
    try {
      pairBtn.disabled = true;
      await api.pair(qrJson);
      if (input) input.value = '';
      await refreshStatus();
      showScreen('library-screen');
      await refreshLibrary(true);
      showToast('Dispositivo pareado com sucesso!');
    } catch (err) {
      showToast(`Falha no pareamento: ${err}`);
    } finally {
      pairBtn.disabled = false;
    }
  });

  // Tela de pareamento (atalho Desktop): configurar pasta de livros e auto-parear em loopback
  const pairVaultBtn = document.getElementById('btn-pairing-save-vault');
  pairVaultBtn?.addEventListener('click', async () => {
    const vaultInput = document.getElementById('pairing-vault-input');
    const vaultDir = vaultInput?.value?.trim() || '';
    if (!vaultDir) {
      showToast('Informe o caminho da pasta de livros.');
      return;
    }
    try {
      pairVaultBtn.disabled = true;
      await api.configureServerVault(vaultDir);
      await refreshStatus();
      showScreen('library-screen');
      await refreshLibrary(true);
      showToast('Servidor local iniciado e pareado automaticamente!');
    } catch (err) {
      showToast(`${err}`);
    } finally {
      pairVaultBtn.disabled = false;
    }
  });

  // Controles do Leitor
  document.getElementById('btn-reader-back')?.addEventListener('click', () => {
    closeActiveReader();
  });
  document.getElementById('btn-reader-prev')?.addEventListener('click', () => {
    appState.activeReader?.prev?.();
  });
  document.getElementById('btn-reader-next')?.addEventListener('click', () => {
    appState.activeReader?.next?.();
  });
  appState.progressBar = createProgressBar({
    getReader: () => appState.activeReader,
    bookId: () => appState.activeBook?.book_id || null,
  });
  document.getElementById('btn-zoom-out')?.addEventListener('click', () => {
    appState.activeReader?.zoomOut?.();
  });
  document.getElementById('btn-zoom-in')?.addEventListener('click', () => {
    appState.activeReader?.zoomIn?.();
  });
  document.getElementById('btn-fit-toggle')?.addEventListener('click', (ev) => {
    const mode = appState.activeReader?.toggleFitMode?.();
    if (mode && ev.currentTarget) {
      ev.currentTarget.textContent = mode === 'width' ? 'Ajustar: Largura' : 'Ajustar: Página';
    }
  });
  document.getElementById('btn-reader-toc')?.addEventListener('click', () => {
    document.getElementById('toc-drawer')?.classList.toggle('hidden');
  });
  document.getElementById('btn-close-toc')?.addEventListener('click', () => {
    document.getElementById('toc-drawer')?.classList.add('hidden');
  });

  // Atalhos de teclado no leitor
  window.addEventListener('keydown', (ev) => {
    if (!appState.activeReader) return;
    if (ev.key === 'Escape' && appState.aaPanel?.isOpen()) {
      ev.preventDefault();
      appState.aaPanel.close();
      return;
    }
    const tag = ev.target?.tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') return;
    if (ev.key === 'ArrowRight' || ev.key === 'PageDown') {
      ev.preventDefault();
      appState.activeReader.next?.();
    } else if (ev.key === 'ArrowLeft' || ev.key === 'PageUp') {
      ev.preventDefault();
      appState.activeReader.prev?.();
    } else if (ev.key === 'Escape') {
      ev.preventDefault();
      closeActiveReader();
    }
  });

  // Modal de Configurações
  const settingsModal = document.getElementById('settings-modal');
  document.getElementById('btn-open-settings')?.addEventListener('click', async () => {
    await refreshStatus();
    settingsModal?.classList.remove('hidden');
  });
  document.getElementById('btn-open-settings-from-pair')?.addEventListener('click', async () => {
    await refreshStatus();
    settingsModal?.classList.remove('hidden');
  });
  document.getElementById('btn-close-settings')?.addEventListener('click', () => {
    settingsModal?.classList.add('hidden');
  });

  // Salvar pasta de livros no Desktop
  const saveVaultBtn = document.getElementById('btn-settings-save-vault');
  saveVaultBtn?.addEventListener('click', async () => {
    const vaultInput = document.getElementById('settings-vault-input');
    const vaultDir = vaultInput?.value?.trim() || '';
    if (!vaultDir) {
      showToast('Digite o caminho da pasta de livros.');
      return;
    }
    try {
      saveVaultBtn.disabled = true;
      const st = await api.configureServerVault(vaultDir);
      appState.status = st;
      await renderDesktopServerPanel(st.embedded_server);
      showScreen('library-screen');
      await refreshLibrary(true);
      showToast('Pasta de livros configurada e sincronizada!');
    } catch (err) {
      showToast(`${err}`);
    } finally {
      saveVaultBtn.disabled = false;
    }
  });

  // Rotacionar segredo/QR de pareamento
  document.getElementById('btn-rotate-qr')?.addEventListener('click', async () => {
    try {
      const qrDto = await api.rotatePairingQr();
      updatePairingQrDisplay(qrDto);
      showToast('Novo QR de pareamento gerado.');
    } catch (err) {
      showToast(`Erro ao gerar novo QR: ${err}`);
    }
  });

  // Copiar JSON do QR
  document.getElementById('btn-copy-qr-json')?.addEventListener('click', async () => {
    const qrInput = document.getElementById('server-qr-json');
    const val = qrInput?.value || '';
    if (!val) return;
    try {
      await navigator.clipboard.writeText(val);
      showToast('JSON do QR copiado para a área de transferência.');
    } catch (_) {
      qrInput?.select?.();
      showToast('Selecione e copie o JSON exibido no campo.');
    }
  });

  // Salvar preferências de leitura (fonte, tema)
  document.getElementById('btn-save-preferences')?.addEventListener('click', async () => {
    const fontSize = Number(document.getElementById('settings-font-size')?.value || 18);
    const theme = document.getElementById('settings-theme-select')?.value || 'auto';
    try {
      await api.saveSettings({
        defaultFontSize: fontSize,
        theme,
      });
      applyTheme(theme);
      appState.activeReader?.setAppTheme?.(theme);
      await refreshStatus();
      showToast('Preferências salvas.');
    } catch (err) {
      showToast(`Erro ao salvar preferências: ${err}`);
    }
  });

  // Desparear dispositivo
  document.getElementById('btn-unpair-device')?.addEventListener('click', async () => {
    try {
      await api.unpair();
      settingsModal?.classList.add('hidden');
      if (appState.activeReader) {
        await appState.activeReader.destroy();
        appState.activeReader = null;
      }
      await refreshStatus();
      showScreen('pairing-screen');
      showToast('Dispositivo despareado.');
    } catch (err) {
      showToast(`Erro ao desparear: ${err}`);
    }
  });

  // Ao voltar ao primeiro plano (visibilitychange), dispara flush de posição e on_resume
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      appState.activeReader?.flush?.();
    } else if (document.visibilityState === 'visible') {
      api.onResume().catch(() => {});
    }
  });
}

async function setupClientEventStream() {
  await listenClientEvents((ev) => {
    if (!ev || typeof ev !== 'object') return;

    // `ClientEvent` é serializado por serde como enum externamente marcado:
    // { DownloadProgress: { book_id, bytes, total } }, { PositionChanged: { book_id, position, origin } }, etc.
    if (ev.DownloadProgress) {
      const d = ev.DownloadProgress;
      downloadModal.update(d.bytes || 0, d.total || 0, formatBytes);
    } else if (ev.Connected) {
      appState.isStale = false;
      updateSyncIndicator('connected', false);
    } else if (ev.Disconnected) {
      updateSyncIndicator('offline', appState.isStale);
    } else if (ev.Unpaired) {
      showToast('Pareamento revogado pelo servidor.');
      refreshStatus().then(() => {
        showScreen('pairing-screen');
      });
    } else if (ev.PositionChanged) {
      const { book_id: bookId, position: pos } = ev.PositionChanged;
      if (!pos) return;

      const idx = appState.books.findIndex((b) => b.book_id === bookId);
      if (idx >= 0) {
        appState.books[idx] = { ...appState.books[idx], position: pos };
      }

      // Se estamos na tela da biblioteca, atualiza os cards imediatamente
      if (!appState.activeBook) {
        renderLibraryView(
          appState.books,
          appState.searchQuery,
          (book) => openBookFlow(book),
          (book) => removeBookDownloadFlow(book),
        );
      } else if (
        appState.activeBook.book_id === bookId &&
        pos.device_id !== appState.status?.device_id
      ) {
        // Outro dispositivo avançou no livro atualmente aberto: exibe toast sem arrancar a página!
        const devName = resolveDeviceLabel(pos);
        const pctLabel = formatPercent(pos.percent);
        showToast(`Ir para a posição de ${devName} (${pctLabel})?`, {
          actionLabel: 'Ir para lá',
          durationMs: 12000,
          onAction: () => {
            appState.activeReader?.goToRemotePosition?.(pos);
          },
        });
      }
    }
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Executa o roteiro de verificação E2E real quando `status.e2e_mode === true` (BOOKFERRY_E2E=1).
 */
async function runE2eAutomation() {
  const results = {
    started_at: new Date().toISOString(),
    ok: false,
    steps: {},
    screenshots: [],
    error: null,
  };

  try {
    await sleep(600);
    await refreshLibrary(true);
    await sleep(500);

    const formatsFound = appState.books.map((b) => b.format).sort();
    results.steps.library_loaded = {
      count: appState.books.length,
      formats: formatsFound,
    };

    const epubBook = appState.books.find((b) => b.format === 'epub');
    const cbzBook = appState.books.find((b) => b.format === 'cbz');
    const pdfBook = appState.books.find((b) => b.format === 'pdf');
    if (!epubBook || !cbzBook || !pdfBook) {
      throw new Error(
        `Faltam formatos na biblioteca E2E: encontrados=${JSON.stringify(formatsFound)}`,
      );
    }

    // Pareia o segundo cliente ("Celular S23 E2E") no mesmo servidor embutido
    const peerInfo = await api.e2ePairSecondClient('Celular S23 E2E');
    registerKnownDeviceName(peerInfo.device_id, peerInfo.device_name);
    results.steps.second_client_paired = peerInfo;
    await sleep(400);

    // Screenshot 1: Biblioteca inicial com os 3 formatos
    results.screenshots.push(await api.e2eCaptureScreenshot('01-library'));

    // Navegacao por pastas: so aparecem pastas com livros (Jogos/ e Vazia/ nao); entra em
    // Quadrinhos > DC, confere as migalhas e volta pela raiz.
    const rootFolders = [...document.querySelectorAll('#folder-grid .folder-card')].map(
      (el) => el.dataset.folderPath,
    );
    results.steps.folders_at_root = rootFolders;
    if (JSON.stringify(rootFolders) !== JSON.stringify(['Manuais', 'Quadrinhos'])) {
      throw new Error(`Pastas na raiz inesperadas: ${JSON.stringify(rootFolders)}`);
    }
    document.querySelector('#folder-grid .folder-card[data-folder-path="Quadrinhos"]')?.click();
    await sleep(200);
    document.querySelector('#folder-grid .folder-card[data-folder-path="Quadrinhos/DC"]')?.click();
    await sleep(300);
    const crumbText = document.getElementById('folder-breadcrumb')?.textContent || '';
    const cbzCards = [...document.querySelectorAll('#library-grid .book-card')].map(
      (el) => el.dataset.format,
    );
    results.steps.folder_navigation = { breadcrumb: crumbText, cards: cbzCards };
    if (!crumbText.includes('Quadrinhos') || !crumbText.includes('DC') || cbzCards.join() !== 'cbz') {
      throw new Error(`Navegacao por pastas falhou: ${JSON.stringify(results.steps.folder_navigation)}`);
    }
    results.screenshots.push(await api.e2eCaptureScreenshot('09-folder-nav'));
    document.querySelector('#folder-breadcrumb button.crumb')?.click(); // "Biblioteca" (raiz)
    await sleep(200);

    // 1. Fluxo EPUB: abrir, virar página, aguardar debounce + SSE no celular, testar toast remoto
    await openBookFlow(epubBook);
    await sleep(1000);
    await appState.activeReader.next();
    await sleep(1800); // Aguarda o debounce real de 1,5s + propagação SSE

    const epubPeerPos = await api.e2eWaitPeerPosition(epubBook.book_id, 0.4, 6000, 'epubcfi(');
    results.steps.epub_sse_to_peer = epubPeerPos;
    results.screenshots.push(await api.e2eCaptureScreenshot('02-reader-epub'));

    // 1b. Painel "Aa": muda estilo ao vivo, confere no DOM do livro, confirma que o ponto de
    // leitura (CFI) não mudou e que as preferências persistem por livro.
    {
      const aa = {};
      const setRange = (id, value) => {
        const el = document.getElementById(id);
        el.value = String(value);
        el.dispatchEvent(new Event('input', { bubbles: true }));
      };
      const setSelect = (id, value) => {
        const el = document.getElementById(id);
        el.value = value;
        el.dispatchEvent(new Event('change', { bubbles: true }));
      };
      const bookStyle = () => {
        const doc = appState.activeReader.getContentDoc();
        const cs = doc.defaultView.getComputedStyle(doc.body);
        return {
          fontSize: cs.fontSize,
          lineHeight: cs.lineHeight,
          background: cs.backgroundColor,
          color: cs.color,
          fontFamily: cs.fontFamily,
          gap: document.querySelector('foliate-view').renderer.getAttribute('gap'),
        };
      };
      const locatorBefore = appState.activeReader.getCurrentState().locator;
      document.getElementById('btn-reader-aa').click();
      await sleep(200);
      aa.panel_open = !document.getElementById('aa-panel').classList.contains('hidden');
      aa.before = bookStyle();

      setRange('aa-font-size', 26);
      setRange('aa-line-height', 2);
      setRange('aa-margin', 12);
      setSelect('aa-font-family', 'mono');
      setSelect('aa-align', 'left');
      document.querySelector('.aa-theme[data-theme="sepia"]').click();
      await sleep(1500);
      aa.after_sepia = bookStyle();
      results.screenshots.push(await api.e2eCaptureScreenshot('10-epub-aa-panel'));

      document.querySelector('.aa-theme[data-theme="black"]').click();
      await sleep(1200);
      aa.after_black = bookStyle();
      aa.chrome_theme = document.getElementById('reader-screen').dataset.readerTheme;
      aa.chrome_bg = getComputedStyle(document.getElementById('reader-screen')).backgroundColor;
      results.screenshots.push(await api.e2eCaptureScreenshot('11-epub-black'));

      const locatorAfter = appState.activeReader.getCurrentState().locator;
      aa.cfi_unchanged = locatorBefore === locatorAfter;
      aa.anchor_visible = appState.activeReader.anchorIsVisible(locatorBefore);

      // Fecha o painel com Esc e o leitor; espera o salvamento por livro.
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
      await sleep(150);
      aa.panel_closed_by_esc = document.getElementById('aa-panel').classList.contains('hidden');
      aa.reader_still_open = Boolean(appState.activeReader);
      await sleep(600);
      await closeActiveReader();
      await sleep(400);
      await openBookFlow(epubBook);
      await sleep(1500);
      aa.after_reopen = bookStyle();
      aa.reopen_cfi_ok = appState.activeReader.anchorIsVisible(locatorBefore);
      const other = await api.getReaderPrefs(pdfBook.book_id);
      const own = await api.getReaderPrefs(epubBook.book_id);
      aa.other_book_is_custom = other.is_custom;
      aa.other_book_font_size = other.prefs.font_size;
      aa.own_book_is_custom = own.is_custom;
      aa.own_book_font_size = own.prefs.font_size;
      aa.footer_text = document.getElementById('reader-status-label').textContent;
      aa.footer_valuenow = Number(document.getElementById('reader-progress').getAttribute('aria-valuenow'));
      results.steps.epub_aa_panel = aa;

      const problems = [];
      if (!aa.panel_open) problems.push('painel Aa não abriu');
      if (aa.after_sepia.fontSize !== '26px') problems.push(`tamanho ${aa.after_sepia.fontSize}`);
      if (aa.after_sepia.lineHeight !== '52px') problems.push(`entrelinha ${aa.after_sepia.lineHeight}`);
      if (aa.after_sepia.gap !== '12%') problems.push(`margem ${aa.after_sepia.gap}`);
      if (!/mono|Mono|Consolas|Menlo/.test(aa.after_sepia.fontFamily)) problems.push('fonte mono');
      if (aa.after_sepia.background !== 'rgb(245, 236, 215)') problems.push(`sépia ${aa.after_sepia.background}`);
      if (aa.after_black.background !== 'rgb(0, 0, 0)') problems.push(`preto ${aa.after_black.background}`);
      if (aa.chrome_theme !== 'black' || aa.chrome_bg !== 'rgb(0, 0, 0)') problems.push('chrome não ficou preto');
      if (!aa.cfi_unchanged || !aa.anchor_visible) problems.push('o ponto de leitura mudou ao mudar o estilo');
      if (!aa.panel_closed_by_esc || !aa.reader_still_open) problems.push('Esc deveria fechar só o painel');
      if (aa.after_reopen.fontSize !== '26px' || aa.after_reopen.background !== 'rgb(0, 0, 0)') {
        problems.push('preferências do livro não persistiram');
      }
      if (!aa.reopen_cfi_ok) problems.push('reabrir não voltou ao mesmo ponto');
      if (aa.other_book_is_custom || aa.other_book_font_size === 26) problems.push('prefs vazaram para outro livro');
      if (!aa.own_book_is_custom) problems.push('livro sem prefs próprias');
      if (!/%/.test(aa.footer_text) || !(aa.footer_valuenow > 0)) problems.push(`rodapé EPUB: ${aa.footer_text}`);
      if (problems.length) throw new Error(`Painel Aa: ${problems.join('; ')} :: ${JSON.stringify(aa)}`);
    }

    // O "celular" escreve posição mais nova no livro EPUB aberto -> app deve mostrar toast
    const epubState = appState.activeReader.getCurrentState();
    await api.e2ePeerSetPosition(
      epubBook.book_id,
      epubState.locator || 'epubcfi(/6/4!/4/2/1:0)',
      88.0,
      'Capítulo 3 (Remoto)',
    );
    await sleep(900);
    const toastVisible = !document.getElementById('toast-container')?.classList.contains('hidden');
    const toastText = document.getElementById('toast-container')?.textContent || '';
    results.steps.remote_toast_shown = { visible: toastVisible, text: toastText };
    results.screenshots.push(await api.e2eCaptureScreenshot('03-remote-toast'));
    hideToast();

    await closeActiveReader();
    await sleep(400);

    // 2. Fluxo CBZ: abrir, avançar página, aguardar debounce + SSE no celular
    await openBookFlow(cbzBook);
    await sleep(700);
    await appState.activeReader.next();
    await sleep(1800);

    const cbzPeerPos = await api.e2eWaitPeerPosition(cbzBook.book_id, 25.0, 6000, 'page:2');
    results.steps.cbz_sse_to_peer = cbzPeerPos;
    results.screenshots.push(await api.e2eCaptureScreenshot('04-reader-cbz'));
    {
      const label = document.getElementById('reader-status-label');
      const bar = document.getElementById('reader-progress');
      const cbz = { text_fmt0: label.textContent };
      label.click();
      cbz.text_fmt1 = label.textContent;
      label.click();
      cbz.text_fmt2 = label.textContent;
      label.click();
      const rect = bar.getBoundingClientRect();
      const fire = (type, x) =>
        bar.dispatchEvent(new PointerEvent(type, { bubbles: true, clientX: x, clientY: rect.top + 5, pointerId: 1 }));
      fire('pointerdown', rect.right - 1);
      fire('pointerup', rect.right - 1);
      await sleep(400);
      cbz.after_seek_end = label.textContent;
      fire('pointerdown', rect.left + 1);
      fire('pointerup', rect.left + 1);
      await sleep(400);
      cbz.after_seek_start = label.textContent;
      results.steps.cbz_progress_bar = cbz;
      if (!/^Pág\. 2 \/ 6$/.test(cbz.text_fmt0) || !/%$/.test(cbz.text_fmt1) || !/falta/.test(cbz.text_fmt2)) {
        throw new Error(`Rodapé CBZ: ${JSON.stringify(cbz)}`);
      }
      if (cbz.after_seek_end !== 'Pág. 6 / 6' || cbz.after_seek_start !== 'Pág. 1 / 6') {
        throw new Error(`Seek pela barra (CBZ): ${JSON.stringify(cbz)}`);
      }
      // Volta para a página 2: o resto do roteiro depende da posição salva.
      await appState.activeReader.seekPage(2);
      await sleep(1800);
    }

    await closeActiveReader();
    await sleep(400);

    // 3. Fluxo Resume Ask Modal + Leitor PDF:
    // Define posição local = 20% (pág. 1) e posição remota no "celular" = 60% (pág. 3)
    await api.setPosition(pdfBook.book_id, 'page:1', 20.0, 'Página 1 de 5');
    await sleep(300);
    await api.e2ePeerSetPosition(pdfBook.book_id, 'page:3', 60.0, 'Página 3 de 5');
    await sleep(700);

    // Dispara abertura do PDF (vai parar no modal `Ask`)
    const openPdfPromise = openBookFlow(pdfBook);
    await sleep(700);

    const resumeModalEl = document.getElementById('resume-modal');
    const modalVisible = Boolean(resumeModalEl && !resumeModalEl.classList.contains('hidden'));
    const modalText = document.getElementById('resume-modal-message')?.textContent || '';
    results.steps.resume_ask_modal = { visible: modalVisible, text: modalText };
    results.screenshots.push(await api.e2eCaptureScreenshot('05-resume-modal'));

    // Clica em "Continuar lá" para abrir na página 3 (60%)
    document.getElementById('resume-btn-remote')?.click();
    await openPdfPromise;
    await sleep(900);

    // Avança para a página 4 (80%) no PDF e confirma chegada via SSE no celular
    await appState.activeReader.next();
    await sleep(1800);
    const pdfPeerPos = await api.e2eWaitPeerPosition(pdfBook.book_id, 75.0, 6000, 'page:4');
    results.steps.pdf_sse_to_peer = pdfPeerPos;
    results.screenshots.push(await api.e2eCaptureScreenshot('06-reader-pdf'));

    await closeActiveReader();
    await sleep(500);

    // Screenshot da biblioteca atualizada com barras de progresso e "Continuar lendo"
    results.screenshots.push(await api.e2eCaptureScreenshot('07-library-updated'));

    // Abre modal de configurações mostrando o QR SVG e dispositivos pareados
    document.getElementById('btn-open-settings')?.click();
    await sleep(700);
    await refreshPairedDevicesList();
    await sleep(300);
    results.screenshots.push(await api.e2eCaptureScreenshot('08-settings-qr'));

    results.ok = true;
    await api.e2eFinish(results);
  } catch (err) {
    results.error = `${err?.message || err} | ${err?.stack || ''}`;
    await api.e2eFinish(results);
  }
}

async function initApp() {
  setupEventListeners();
  await setupClientEventStream();

  try {
    const st = await refreshStatus();
    if (st.paired) {
      showScreen('library-screen');
      await refreshLibrary(true);
    } else {
      showScreen('pairing-screen');
    }

    if (st.e2e_mode) {
      runE2eAutomation();
    }
  } catch (err) {
    showToast(`Falha ao inicializar aplicativo: ${err}`);
  }
}

window.addEventListener('DOMContentLoaded', initApp);
