// Ponto de entrada do frontend Bookferry (Tauri v2 + ES Modules).

import { api, listenClientEvents, setProtocolBaseUrl } from './api.js';
import {
  getCurrentFolder,
  navigateToFolder,
  prepareBookToOpen,
  renderLibraryView,
  updateSyncIndicator,
} from './library.js';
import { mountCbzReader } from './readers/cbz.js';
import { mountEpubReader } from './readers/epub.js';
import { mountPdfReader } from './readers/pdf.js';
import { updateBookMenu } from './book-menu.js';
import { generateCoverIfMissing, generateMissingCovers, resizeImageToJpeg } from './covers.js';
import { decodeQrFromImageData, grabFrame, scanQrFromCamera } from './qr-scanner.js';
import { createReaderChrome } from './reader-chrome.js';
import { createProgressBar, mountAaPanel, setReaderChromeTheme } from './reader-ui.js';
import { mountShortcutsUi } from './shortcuts.js';
import {
  bookDisplayTitle,
  DEFAULT_READER_PREFS,
  cameraErrorMessage,
  formatBytes,
  formatPercent,
  isHiddenFromContinue,
  parentFolderPath,
  parsePairingPayload,
  resolveLibraryKeyAction,
  sanitizeProgressPrefs,
} from './state.js';
import {
  applyTheme,
  downloadModal,
  hideToast,
  refreshPairedDevicesList,
  registerKnownDeviceName,
  renderDesktopServerPanel,
  resolveDeviceLabel,
  setButtonLabel,
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
  chrome: null,
  shortcuts: null,
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

// Reflete no checkbox o estado REAL do início com o computador (o arquivo de autostart do
// sistema), e avisa nos sistemas em que essa parte ainda não existe.
async function refreshAutostartControl() {
  const box = document.getElementById('settings-autostart');
  const note = document.getElementById('settings-autostart-note');
  if (!box) return;
  try {
    const st = await api.getAutostart();
    box.checked = !!st.enabled;
    box.disabled = !st.supported;
    note?.classList.toggle('hidden', !!st.supported);
  } catch (_) {
    box.disabled = true;
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

  // Segundo plano (bandeja e início com o computador): só existe no desktop.
  const backgroundSection = document.getElementById('settings-desktop-background');
  backgroundSection?.classList.toggle('hidden', !isDesktop);
  if (isDesktop) {
    const closeToTray = document.getElementById('settings-close-to-tray');
    if (closeToTray) closeToTray.checked = st.settings?.close_to_tray !== false;
    await refreshAutostartControl();
  }

  // Leitura de QR pela câmera só no celular (o webview do desktop não expõe a câmera).
  document.getElementById('btn-scan-qr')?.classList.toggle('hidden', isDesktop);

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
    // Só o desktop gera capas em segundo plano (tem o servidor e fica ligado); o celular gera
    // a capa de um livro quando o abre. Um livro por vez, sem travar a tela.
    if (appState.status?.platform === 'desktop' && !view?.stale) {
      generateMissingCovers(() => appState.books);
    }
  } catch (err) {
    updateSyncIndicator('offline', true);
    showToast(`Erro ao carregar biblioteca: ${err}`);
  }
}

async function removeBookDownloadFlow(book) {
  try {
    await api.removeDownload(book.book_id);
    showToast(`Cópia local de "${bookDisplayTitle(book)}" removida.`);
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

let comicPrefsTimer = null;

/** Salva por livro (debounce) as preferências de quadrinhos mudadas pela toolbar. */
function saveComicPrefs(bookId, prefs) {
  if (comicPrefsTimer) clearTimeout(comicPrefsTimer);
  comicPrefsTimer = setTimeout(() => {
    comicPrefsTimer = null;
    api.saveReaderPrefs(bookId, prefs).catch(() => {});
  }, 400);
}

const FIT_LABELS = { page: 'Página', width: 'Largura', height: 'Altura' };
const SPREAD_LABELS = { single: '1 página', double: '2 páginas', auto: 'Automático' };

/** Atualiza os rótulos dos botões de quadrinhos conforme as preferências do leitor ativo. */
function refreshComicButtons() {
  const reader = appState.activeReader;
  if (reader?.format !== 'cbz') return;
  const p = reader.getPrefs();
  const $ = (id) => document.getElementById(id);
  const rtl = p.direction === 'rtl';
  setButtonLabel($('btn-comic-direction'), rtl ? 'Mangá' : 'Ocidental', rtl ? 'i-arrow-left' : 'i-arrow-right');
  $('btn-comic-direction')?.setAttribute('title', rtl
    ? 'Leitura da direita para a esquerda (mangá). Clique para inverter.'
    : 'Leitura da esquerda para a direita. Clique para mangá (direita → esquerda).');
  setButtonLabel(
    $('btn-comic-spread'),
    SPREAD_LABELS[p.spread_mode] || SPREAD_LABELS.single,
    p.spread_mode === 'single' ? 'i-page' : 'i-book-open',
  );
  setButtonLabel($('btn-comic-cover'), p.cover_alone ? 'Capa sozinha' : 'Capa em par');
  setButtonLabel($('btn-comic-margin'), p.zero_margin ? 'Sem margem' : 'Com margem');
  $('btn-comic-margin')?.setAttribute('aria-pressed', String(Boolean(p.zero_margin)));
  setButtonLabel($('btn-fit-toggle'), FIT_LABELS[p.fit] || FIT_LABELS.page);
  // "Capa" só faz sentido quando há duas páginas na tela.
  $('btn-comic-cover')?.classList.toggle('hidden', p.spread_mode === 'single');
}

function updateReaderToolbarForFormat(format) {
  const isEpub = format === 'epub';
  document.getElementById('reader-epub-controls')?.classList.toggle('hidden', !isEpub);
  document.getElementById('reader-zoom-controls')?.classList.toggle('hidden', isEpub);
  document.getElementById('reader-comic-controls')?.classList.toggle('hidden', format !== 'cbz');
  document.getElementById('btn-reader-toc')?.classList.toggle('hidden', !isEpub);
}

export async function openBookFlow(book) {
  const prepared = await prepareBookToOpen(book);
  if (!prepared) return;

  const { book: readyBook, initialPosition } = prepared;
  appState.activeBook = readyBook;

  // Abrir um livro tirado de "Continuar lendo" o traz de volta na hora. Não dá para depender da
  // posição salva: ela só muda quando a página muda, e quem só abre e fecha não mexeria nela.
  const listedBook = appState.books.find((b) => b.book_id === readyBook.book_id);
  if (isHiddenFromContinue(listedBook || readyBook)) {
    api.setContinueHidden(readyBook.book_id, false).catch(() => {});
  }

  // Atualiza status de baixado na lista local
  const idx = appState.books.findIndex((b) => b.book_id === readyBook.book_id);
  if (idx >= 0) {
    appState.books[idx] = { ...appState.books[idx], downloaded: true };
  }

  const titleEl = document.getElementById('reader-book-title');
  const statusLabelEl = document.getElementById('reader-status-label');
  const viewportEl = document.getElementById('reader-viewport');
  const tocListEl = document.getElementById('toc-list');

  if (titleEl) titleEl.textContent = bookDisplayTitle(readyBook);
  // Abriu um livro sem capa: já está baixado, então gera a capa embutida sem esperar.
  generateCoverIfMissing(idx >= 0 ? appState.books[idx] : readyBook);
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
    const chrome = appState.chrome;
    const hooks = {
      onProgress: (info) => bar?.update(info),
      onToggleChrome: () => chrome?.toggleImmersive(),
      onKey: (ev) => chrome?.handleKey(ev),
      onWheel: (ev) => chrome?.handleWheel(ev),
      onDismissOverlays: () => appState.aaPanel?.close?.({ restoreFocus: false }),
    };

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
        prefs: prefsView?.prefs,
        viewportEl,
        statusLabelEl,
        onSavePosition: saveReaderPosition,
        hooks: {
          ...hooks,
          onPrefsChange: (p) => {
            refreshComicButtons();
            saveComicPrefs(readyBook.book_id, p);
          },
        },
      });
      refreshComicButtons();
    } else {
      throw new Error(`Formato não suportado: ${readyBook.format}`);
    }
    appState.chrome?.open();
  } catch (err) {
    showToast(`Erro ao abrir leitor: ${err}`);
    await closeActiveReader();
  }
}

async function teardownReader() {
  await appState.chrome?.close();
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

/** Atalhos da biblioteca (desktop): só com a biblioteca à vista, sem janela aberta nem campo em foco. */
function handleLibraryKey(ev) {
  if (ev.defaultPrevented) return;
  if (document.getElementById('library-screen')?.classList.contains('hidden')) return;
  if (document.querySelector('.modal-backdrop:not(.hidden)')) return;
  const tag = ev.target?.tagName;
  if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA' || ev.target?.isContentEditable) {
    return;
  }
  switch (resolveLibraryKeyAction(ev)) {
    case 'search': {
      ev.preventDefault();
      const input = document.getElementById('library-search-input');
      input?.focus();
      input?.select();
      break;
    }
    case 'refresh':
      ev.preventDefault(); // F5/Ctrl+R recarregariam o app inteiro
      document.getElementById('btn-refresh-library')?.click();
      break;
    case 'settings':
      ev.preventDefault();
      document.getElementById('btn-open-settings')?.click();
      break;
    case 'help':
      ev.preventDefault();
      appState.shortcuts?.openHelp();
      break;
    case 'up':
      // Busca ativa mostra todas as pastas; só sobe quando há pasta aberta.
      if (!appState.searchQuery && getCurrentFolder() !== '') {
        ev.preventDefault();
        navigateToFolder(parentFolderPath(getCurrentFolder()));
      }
      break;
    default:
      break;
  }
}

function setupEventListeners() {
  // "Remover cópia local" pelo menu do livro (book-menu.js)
  window.addEventListener('bookferry:remove-download', (ev) => {
    if (ev.detail?.book_id) removeBookDownloadFlow(ev.detail);
  });

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

  // Logo: volta ao início da biblioteca (raiz, sem busca, no topo)
  document.getElementById('btn-home')?.addEventListener('click', () => {
    appState.searchQuery = '';
    if (searchInput) searchInput.value = '';
    navigateToFolder('');
    renderLibraryView(
      appState.books,
      appState.searchQuery,
      (book) => openBookFlow(book),
      (book) => removeBookDownloadFlow(book),
    );
    document.getElementById('library-scroll-area')?.scrollTo?.({ top: 0 });
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

  // Tela de pareamento: colar JSON do QR ou lê-lo pela câmera
  const pairBtn = document.getElementById('btn-submit-pair');
  const pairWithJson = async (qrJson) => {
    const input = document.getElementById('pairing-qr-input');
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
  };
  pairBtn?.addEventListener('click', async () => {
    const input = document.getElementById('pairing-qr-input');
    const qrJson = input?.value?.trim() || '';
    if (!qrJson) {
      showToast('Cole o JSON do QR de pareamento.');
      return;
    }
    await pairWithJson(qrJson);
  });

  const scanOverlay = document.getElementById('qr-scan-overlay');
  let scanAbort = null;
  const closeQrScan = () => {
    scanAbort?.abort();
    scanAbort = null;
    scanOverlay?.classList.add('hidden');
  };
  const cancelQrScan = () => {
    // O "voltar" do Android desfaz a entrada de historico criada ao abrir o leitor.
    if (history.state?.bookferryQrScan) history.back();
    else closeQrScan();
  };
  const openQrScan = async () => {
    if (scanAbort) return;
    const abort = new AbortController();
    scanAbort = abort;
    const status = document.getElementById('qr-scan-status');
    const hint = 'Aponte a câmera para o QR Code que aparece em Ajustes no computador.';
    if (status) status.textContent = hint;
    scanOverlay?.classList.remove('hidden');
    history.pushState({ bookferryQrScan: true }, '');
    let qrJson = null;
    try {
      qrJson = await scanQrFromCamera({
        video: document.getElementById('qr-scan-video'),
        canvas: document.getElementById('qr-scan-canvas'),
        accept: parsePairingPayload,
        onIgnored: () => {
          if (status) status.textContent = 'Esse QR Code não é de pareamento do Bookferry. Procure o QR em Ajustes no computador.';
        },
        signal: abort.signal,
      });
    } catch (err) {
      if (err?.name !== 'AbortError') showToast(cameraErrorMessage(err));
    }
    if (scanAbort === abort) {
      closeQrScan();
      if (history.state?.bookferryQrScan) history.back();
    }
    if (qrJson) await pairWithJson(qrJson);
  };
  document.getElementById('btn-scan-qr')?.addEventListener('click', openQrScan);
  document.getElementById('btn-qr-scan-cancel')?.addEventListener('click', cancelQrScan);
  window.addEventListener('popstate', () => {
    if (scanAbort) closeQrScan();
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
  appState.shortcuts = mountShortcutsUi({ getReader: () => appState.activeReader });
  appState.chrome = createReaderChrome({
    api,
    getReader: () => appState.activeReader,
    getAaPanel: () => appState.aaPanel,
    closeReader: () => closeActiveReader(),
    isDesktop: () => appState.status?.platform === 'desktop',
    openHelp: () => appState.shortcuts?.openHelp(),
    openGoto: () => appState.shortcuts?.openGoto(),
  });
  // Roda do mouse nas margens do leitor (fora da página); o que a página já tratou chega com
  // `defaultPrevented` e é ignorado.
  document.getElementById('reader-viewport')?.addEventListener(
    'wheel',
    (ev) => {
      if (appState.activeReader) appState.chrome?.handleWheel(ev);
    },
    { passive: false },
  );
  document.getElementById('btn-reader-fullscreen')?.addEventListener('click', () => {
    appState.chrome.toggleFullscreen();
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
      setButtonLabel(ev.currentTarget, FIT_LABELS[mode] || FIT_LABELS.page);
    }
  });
  const comicAction = (id, method) =>
    document.getElementById(id)?.addEventListener('click', () => {
      appState.activeReader?.[method]?.();
      refreshComicButtons();
    });
  comicAction('btn-comic-direction', 'toggleDirection');
  comicAction('btn-comic-spread', 'cycleSpreadMode');
  comicAction('btn-comic-cover', 'toggleCoverAlone');
  comicAction('btn-comic-margin', 'toggleZeroMargin');
  document.getElementById('btn-reader-toc')?.addEventListener('click', () => {
    document.getElementById('toc-drawer')?.classList.toggle('hidden');
  });
  document.getElementById('btn-close-toc')?.addEventListener('click', () => {
    document.getElementById('toc-drawer')?.classList.add('hidden');
  });

  // Atalhos de teclado no leitor
  window.addEventListener('keydown', (ev) => {
    if (appState.shortcuts?.handleKey(ev)) return; // ajuda ou "Ir para" aberta
    if (!appState.activeReader) {
      handleLibraryKey(ev);
      return;
    }
    appState.chrome?.handleKey(ev);
  });
  document.getElementById('library-search-input')?.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') ev.target.blur();
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

  // Servidor fora da porta fixa: reinicia para tentar voltar a ela
  document.getElementById('btn-retry-port')?.addEventListener('click', async (ev) => {
    const btn = ev.currentTarget;
    btn.disabled = true;
    try {
      const stillFallback = await api.retryPreferredPort();
      const st = await api.status();
      appState.status = st;
      await renderDesktopServerPanel(st.embedded_server);
      showToast(
        stillFallback
          ? 'A porta fixa continua ocupada.'
          : 'Servidor de volta na porta fixa. Copie o novo código de pareamento.',
      );
    } catch (err) {
      showToast(`Erro ao reiniciar o servidor: ${err}`);
    } finally {
      btn.disabled = false;
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

  // Bandeja e início com o computador: valem na hora, sem o botão Salvar.
  document.getElementById('settings-close-to-tray')?.addEventListener('change', async (ev) => {
    const box = ev.target;
    try {
      await api.saveSettings({ closeToTray: box.checked });
      showToast(
        box.checked
          ? 'Fechar a janela agora mantém o Bookferry na bandeja.'
          : 'Fechar a janela agora encerra o Bookferry.',
      );
    } catch (err) {
      box.checked = !box.checked;
      showToast(`Erro ao salvar: ${err}`);
    }
  });
  document.getElementById('settings-autostart')?.addEventListener('change', async (ev) => {
    const box = ev.target;
    try {
      const st = await api.setAutostart(box.checked);
      box.checked = !!st.enabled;
      showToast(
        st.enabled
          ? 'O Bookferry vai iniciar em segundo plano quando você ligar o computador.'
          : 'O Bookferry não inicia mais com o computador.',
      );
    } catch (err) {
      await refreshAutostartControl();
      showToast(`Erro: ${err}`);
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
    } else if (ev.BookMetaChanged) {
      const { book_id: bookId, meta } = ev.BookMetaChanged;
      const idx = appState.books.findIndex((b) => b.book_id === bookId);
      if (idx < 0 || !meta) return;
      appState.books[idx] = {
        ...appState.books[idx],
        display_title: meta.display_title ?? null,
        cover_version: Number(meta.cover_version) || 0,
        cover_source: meta.cover_source ?? null,
        hidden_continue_at_ms: meta.hidden_continue_at_ms ?? null,
      };
      updateBookMenu(appState.books[idx]);
      // "Restaurar capa original" (aqui ou em outro aparelho): o desktop gera a embutida de novo.
      if (appState.status?.platform === 'desktop' && appState.books[idx].cover_version === 0) {
        generateMissingCovers(() => appState.books);
      }
      if (appState.activeBook?.book_id === bookId) {
        const titleEl = document.getElementById('reader-book-title');
        if (titleEl) titleEl.textContent = bookDisplayTitle(appState.books[idx]);
      }
      if (!appState.activeBook) {
        renderLibraryView(
          appState.books,
          appState.searchQuery,
          (book) => openBookFlow(book),
          (book) => removeBookDownloadFlow(book),
        );
      }
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

    // Sanidade de layout: pega CSS/webview quebrado (texto minúsculo, sem padding, cards
    // colapsados) que nenhum teste de lógica percebe. Valores bem abaixo do normal de propósito.
    {
      const topbar = document.querySelector('#library-screen .topbar');
      const folderCard = document.querySelector('#folder-grid .folder-card');
      const searchBox = document.querySelector('#library-screen input[type="search"], #library-screen input[type="text"]');
      const layout = {
        body_font_px: parseFloat(getComputedStyle(document.body).fontSize),
        topbar_pad_left_px: topbar ? parseFloat(getComputedStyle(topbar).paddingLeft) : 0,
        folder_card_w: folderCard ? Math.round(folderCard.getBoundingClientRect().width) : 0,
        folder_card_h: folderCard ? Math.round(folderCard.getBoundingClientRect().height) : 0,
        search_w: searchBox ? Math.round(searchBox.getBoundingClientRect().width) : 0,
        viewport_w: window.innerWidth,
        dpr: window.devicePixelRatio,
      };
      results.steps.layout_sanity = layout;
      const bad = [];
      if (!(layout.body_font_px >= 12)) bad.push('fonte do corpo < 12px');
      if (!(layout.topbar_pad_left_px >= 8)) bad.push('barra superior sem padding');
      if (!(layout.folder_card_w >= 100 && layout.folder_card_h >= 40)) bad.push('card de pasta colapsado');
      if (!(layout.search_w >= 120)) bad.push('campo de busca colapsado');
      if (bad.length) throw new Error(`Layout quebrado: ${bad.join('; ')} :: ${JSON.stringify(layout)}`);
    }

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
    // Atalhos da biblioteca (estamos em Quadrinhos/DC): subir pasta, buscar, atualizar, ajustes, ajuda.
    {
      const L = {};
      const press = (k, extra = {}, target = window) => {
        const ev = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra });
        target.dispatchEvent(ev);
        return ev.defaultPrevented;
      };
      const si = document.getElementById('library-search-input');
      const helpEl = document.getElementById('shortcuts-modal');
      press('Backspace');
      await sleep(250);
      L.after_backspace = getCurrentFolder();
      press('ArrowLeft', { altKey: true });
      await sleep(250);
      L.after_alt_left = getCurrentFolder();
      L.backspace_at_root_ignored = press('Backspace') === false;
      L.slash_consumed = press('/');
      L.slash_focuses_search = document.activeElement === si;
      press('?', {}, si); // digitando no campo: "?" é texto, não abre a ajuda
      L.help_closed_while_typing = helpEl.classList.contains('hidden');
      si.blur();
      L.f5_prevented = press('F5'); // sem preventDefault o webview recarregaria o app inteiro
      await sleep(500);
      press(',', { ctrlKey: true });
      await sleep(700);
      L.settings_opened = !document.getElementById('settings-modal').classList.contains('hidden');
      L.backspace_ignored_under_modal = press('Backspace') === false;
      document.getElementById('btn-close-settings')?.click();
      await sleep(300);
      press('?');
      await sleep(300);
      L.help_opened = !helpEl.classList.contains('hidden');
      results.screenshots.push(await api.e2eCaptureScreenshot('15-shortcuts-help'));
      press('Escape');
      await sleep(300);
      L.help_closed = helpEl.classList.contains('hidden');
      navigateToFolder('Quadrinhos/DC');
      await sleep(300);
      results.steps.library_shortcuts = L;
      const want = {
        after_backspace: 'Quadrinhos',
        after_alt_left: '',
        backspace_at_root_ignored: true,
        slash_consumed: true,
        slash_focuses_search: true,
        help_closed_while_typing: true,
        f5_prevented: true,
        settings_opened: true,
        backspace_ignored_under_modal: true,
        help_opened: true,
        help_closed: true,
      };
      const bad = Object.entries(want).filter(([name, v]) => L[name] !== v);
      if (bad.length) throw new Error(`Atalhos da biblioteca: ${JSON.stringify(bad)} | ${JSON.stringify(L)}`);
    }

    // Tema "Breu" (preto): salva pelos Ajustes, confere que o Rust persistiu e que fundo e logo
    // seguem a variante preta da marca (#000 e tudo em branco). Depois volta ao automático.
    {
      const T = {};
      const saveTheme = async (value) => {
        document.getElementById('btn-open-settings')?.click();
        await sleep(500);
        document.getElementById('settings-theme-select').value = value;
        document.getElementById('btn-save-preferences').click();
        await sleep(900);
      };
      await saveTheme('black');
      const cs = getComputedStyle(document.documentElement);
      T.data_theme = document.documentElement.getAttribute('data-theme');
      T.persisted = appState.status?.settings?.theme;
      T.select_kept = document.getElementById('settings-theme-select').value;
      T.body_bg = getComputedStyle(document.body).backgroundColor;
      T.logo_vars = ['--logo-page', '--logo-page-2', '--logo-ember', '--logo-sea']
        .map((v) => cs.getPropertyValue(v).trim())
        .join(',');
      T.aa_label = document.querySelector('.aa-theme[data-theme="black"]')?.textContent.trim();
      T.toast_bg = getComputedStyle(document.querySelector('.toast-item') || document.body).backgroundColor;
      document.getElementById('btn-close-settings')?.click();
      await sleep(400);
      results.screenshots.push(await api.e2eCaptureScreenshot('16-tema-breu'));
      await saveTheme('auto');
      T.reverted = document.documentElement.getAttribute('data-theme');
      document.getElementById('btn-close-settings')?.click();
      await sleep(300);
      results.steps.theme_breu = T;
      const want = {
        data_theme: 'black',
        persisted: 'black',
        select_kept: 'black',
        body_bg: 'rgb(0, 0, 0)',
        logo_vars: '#ffffff,#ffffff,#ffffff,#ffffff',
        aa_label: 'AaBreu',
        toast_bg: 'rgb(20, 20, 22)',
        reverted: null,
      };
      const bad = Object.entries(want).filter(([name, v]) => T[name] !== v);
      if (bad.length) throw new Error(`Tema Breu: ${JSON.stringify(bad)} | ${JSON.stringify(T)}`);
    }

    // Segundo plano: a seção só existe no desktop; "fechar mantém na bandeja" persiste pelo Rust e
    // "iniciar com o computador" grava/remove o arquivo de autostart de verdade (o run-e2e.sh
    // aponta XDG_CONFIG_HOME para um diretório temporário e confere o arquivo no fim).
    {
      const S = {};
      const flip = async (id, value) => {
        const box = document.getElementById(id);
        box.checked = value;
        box.dispatchEvent(new Event('change', { bubbles: true }));
        await sleep(700);
      };
      document.getElementById('btn-open-settings')?.click();
      await sleep(500);
      const section = document.getElementById('settings-desktop-background');
      S.section_visible = !!section && !section.classList.contains('hidden');
      S.close_to_tray_default = document.getElementById('settings-close-to-tray').checked;
      await flip('settings-close-to-tray', false);
      S.close_to_tray_off = (await api.status()).settings.close_to_tray;
      await flip('settings-close-to-tray', true);
      S.close_to_tray_on = (await api.status()).settings.close_to_tray;
      S.autostart_start = (await api.getAutostart()).enabled;
      await flip('settings-autostart', true);
      S.autostart_on = (await api.getAutostart()).enabled;
      await flip('settings-autostart', false);
      S.autostart_off = (await api.getAutostart()).enabled;
      await flip('settings-autostart', true); // fica ligado: o run-e2e.sh confere o arquivo
      S.autostart_final = (await api.getAutostart()).enabled;
      S.autostart_box = document.getElementById('settings-autostart').checked;
      section?.scrollIntoView({ block: 'center' });
      await sleep(300);
      results.screenshots.push(await api.e2eCaptureScreenshot('17-ajustes-segundo-plano'));
      document.getElementById('btn-close-settings')?.click();
      await sleep(300);
      results.steps.segundo_plano = S;
      const want = {
        section_visible: true,
        close_to_tray_default: true,
        close_to_tray_off: false,
        close_to_tray_on: true,
        autostart_start: false,
        autostart_on: true,
        autostart_off: false,
        autostart_final: true,
        autostart_box: true,
      };
      const bad = Object.entries(want).filter(([name, v]) => S[name] !== v);
      if (bad.length) throw new Error(`Segundo plano: ${JSON.stringify(bad)} | ${JSON.stringify(S)}`);
    }
    // Logo: de dentro de uma pasta e com busca ativa, volta ao início (raiz, busca limpa).
    {
      const si = document.getElementById('library-search-input');
      si.value = 'zzz-nao-existe';
      si.dispatchEvent(new Event('input', { bubbles: true }));
      await sleep(150);
      document.getElementById('btn-home')?.click();
      await sleep(250);
      const home = {
        search: si.value,
        crumbs: document.querySelectorAll('#folder-breadcrumb button.crumb').length,
        root_folders: document.querySelectorAll('#folder-grid .folder-card').length,
      };
      results.steps.logo_home = home;
      if (home.search !== '' || home.root_folders < 1 || (document.getElementById('folder-breadcrumb')?.textContent || '').includes('Quadrinhos')) {
        throw new Error(`Logo nao voltou ao inicio: ${JSON.stringify(home)}`);
      }
    }
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
      if (aa.after_sepia.background !== 'rgb(244, 234, 214)') problems.push(`sépia ${aa.after_sepia.background}`);
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


    // 1c. Modo imersivo, teclado e tela cheia (EPUB)
    {
      const im = {};
      const reader = () => appState.activeReader;
      const screenEl = document.getElementById('reader-screen');
      const toolbarShown = () =>
        getComputedStyle(document.querySelector('.reader-toolbar')).display !== 'none';
      const clickBook = (frac) => {
        const doc = reader().getContentDoc();
        const view = document.querySelector('foliate-view');
        const vr = view.getBoundingClientRect();
        const fr = doc.defaultView.frameElement.getBoundingClientRect();
        const x = vr.left + vr.width * frac - fr.left; // coordenada dentro do iframe
        doc.dispatchEvent(
          new MouseEvent('click', { bubbles: true, cancelable: true, clientX: x, clientY: 100, view: doc.defaultView }),
        );
      };
      await sleep(800); // deixa o layout do EPUB assentar antes de fixar o ponto de leitura
      const anchor = reader().getCurrentState().locator;
      im.toolbar_before = toolbarShown();

      clickBook(0.5);
      await sleep(600); // toque simples espera o possível segundo toque (220 ms)
      im.immersive_after_center_tap = screenEl.classList.contains('immersive');
      im.toolbar_in_immersive = toolbarShown();
      // O reflow do EPUB acontece depois do ResizeObserver; espera o ponto de leitura reaparecer.
      await sleep(1200);
      im.anchor_visible_in_immersive = reader().anchorIsVisible(anchor);
      results.screenshots.push(await api.e2eCaptureScreenshot('12-epub-immersive'));

      clickBook(0.5);
      await sleep(600);
      im.immersive_after_second_tap = screenEl.classList.contains('immersive');
      im.toolbar_after_second_tap = toolbarShown();
      await sleep(1200); // o EPUB refluiu e o ponto de leitura foi restaurado

      // Zona lateral direita vira a página; esquerda volta.
      const before = reader().getCurrentState();
      clickBook(0.9);
      await sleep(1500);
      im.right_tap_turned_page = reader().getCurrentState().locator !== before.locator;
      clickBook(0.1);
      await sleep(1500);

      // Teclado: F alterna imersivo; Esc sai do imersivo SEM fechar o leitor.
      const key = (k, extra = {}) =>
        window.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra }));
      key('f');
      await sleep(200);
      im.immersive_after_f = screenEl.classList.contains('immersive');
      key('Escape');
      await sleep(200);
      im.immersive_after_esc = screenEl.classList.contains('immersive');
      im.reader_open_after_esc = Boolean(reader());
      await sleep(1200); // reflow do EPUB ao sair do imersivo
      const b2 = reader().getCurrentState().locator;
      key('ArrowRight');
      await sleep(1500);
      im.arrow_right_turned = reader().getCurrentState().locator !== b2;
      key(' ', { shiftKey: true });
      await sleep(1500);

      // Roda do mouse, zoom (= tamanho da fonte) e "Ir para %" no EPUB.
      {
        const e = {};
        const doc = reader().getContentDoc();
        const wheelBook = (deltaY, extra = {}) =>
          doc.dispatchEvent(
            new doc.defaultView.WheelEvent('wheel', { deltaY, bubbles: true, cancelable: true, ...extra }),
          );
        const loc = () => reader().getCurrentState().locator;
        const size = () => appState.aaPanel?.getPrefs().font_size;
        const l0 = loc();
        wheelBook(100);
        await sleep(1500);
        const l1 = loc();
        e.wheel_down_turned = l1 !== l0;
        wheelBook(-100);
        await sleep(1500);
        e.wheel_up_turned = loc() !== l1;

        const s0 = size();
        key('+');
        await sleep(1200);
        e.font_after_plus = size() - s0;
        wheelBook(-100, { ctrlKey: true }); // Ctrl+roda para cima amplia
        await sleep(1200);
        e.font_after_ctrl_wheel = size() - s0;
        key('0');
        await sleep(1200);
        e.font_after_zero = size();
        e.font_default = DEFAULT_READER_PREFS.font_size;
        if (s0 !== e.font_default) appState.aaPanel.adjustFontSize(s0 - e.font_default);
        await sleep(1200);

        key('g');
        await sleep(300);
        const gotoModal = document.getElementById('goto-modal');
        e.goto_opened = !gotoModal.classList.contains('hidden');
        e.goto_title = document.getElementById('goto-title').textContent;
        document.getElementById('goto-input').value = '60';
        document.getElementById('goto-form').requestSubmit();
        await sleep(1800);
        e.goto_closed = gotoModal.classList.contains('hidden');
        e.goto_percent = Math.round(reader().getCurrentState().percent);
        results.steps.epub_shortcuts = e;
        const want = {
          wheel_down_turned: true,
          wheel_up_turned: true,
          font_after_plus: 2,
          font_after_ctrl_wheel: 4,
          font_after_zero: e.font_default,
          goto_opened: true,
          goto_title: 'Ir para o percentual',
          goto_closed: true,
        };
        const bad = Object.entries(want).filter(([name, v]) => e[name] !== v);
        if (bad.length || e.goto_percent < 40 || e.goto_percent > 80) {
          throw new Error(`EPUB atalhos/roda: ${JSON.stringify(bad)} | ${JSON.stringify(e)}`);
        }
        await sleep(600);
      }

      // Tela cheia: F11 liga, Esc desliga (sob xvfb sem gerenciador de janelas pode não efetivar).
      key('F11');
      await sleep(600);
      im.fullscreen_after_f11 = await api.isFullscreen();
      key('Escape');
      await sleep(600);
      im.fullscreen_after_esc = await api.isFullscreen();
      im.reader_open_after_fullscreen_esc = Boolean(reader());
      results.steps.epub_immersive = im;

      const problems = [];
      if (!im.toolbar_before) problems.push('toolbar deveria estar visível');
      if (!im.immersive_after_center_tap || im.toolbar_in_immersive) problems.push('toque central não entrou no imersivo');
      if (!im.anchor_visible_in_immersive) problems.push('imersivo moveu o ponto de leitura');
      if (im.immersive_after_second_tap || !im.toolbar_after_second_tap) problems.push('segundo toque não voltou a interface');
      if (!im.right_tap_turned_page) problems.push('zona direita não virou a página');
      if (!im.immersive_after_f) problems.push('tecla F');
      if (im.immersive_after_esc || !im.reader_open_after_esc) problems.push('Esc deveria só sair do imersivo');
      if (!im.arrow_right_turned) problems.push('seta direita não virou a página');
      if (im.fullscreen_after_esc || !im.reader_open_after_fullscreen_esc) problems.push('Esc deveria sair da tela cheia e manter o leitor');
      if (problems.length) throw new Error(`Imersivo: ${problems.join('; ')} :: ${JSON.stringify(im)}`);
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

    {
      const stage = document.querySelector('.cbz-stage');
      const screenEl = document.getElementById('reader-screen');
      const label = document.getElementById('reader-status-label');
      const tapStage = (frac) => {
        const r = stage.getBoundingClientRect();
        const x = r.left + r.width * frac;
        const y = r.top + r.height / 2;
        const opts = { bubbles: true, clientX: x, clientY: y, pointerId: 7 };
        stage.dispatchEvent(new PointerEvent('pointerdown', opts));
        stage.dispatchEvent(new PointerEvent('pointerup', opts));
      };
      const t = {};
      tapStage(0.5);
      await sleep(300);
      t.immersive_after_center = screenEl.classList.contains('immersive');
      tapStage(0.5);
      await sleep(300);
      t.immersive_after_second = screenEl.classList.contains('immersive');
      const p0 = label.textContent;
      tapStage(0.9);
      await sleep(400);
      t.after_right_tap = label.textContent;
      tapStage(0.1);
      await sleep(400);
      t.after_left_tap = label.textContent;
      t.start = p0;
      results.steps.cbz_tap_zones = t;
      if (!t.immersive_after_center || t.immersive_after_second) throw new Error(`CBZ imersivo: ${JSON.stringify(t)}`);
      if (t.after_right_tap !== 'Pág. 3 / 6' || t.after_left_tap !== 'Pág. 2 / 6') {
        throw new Error(`CBZ zonas de toque: ${JSON.stringify(t)}`);
      }
    }

    // Atalhos de teclado e roda do mouse nos quadrinhos (estamos na pág. 2 de 6, LTR, sem zoom).
    {
      const reader = appState.activeReader;
      const stage = document.querySelector('.cbz-stage');
      const label = document.getElementById('reader-status-label');
      const target = () => document.querySelector('.cbz-target');
      const key = (k, extra = {}) =>
        window.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra }));
      const wheel = (deltaY, extra = {}) =>
        stage.dispatchEvent(new WheelEvent('wheel', { deltaY, bubbles: true, cancelable: true, ...extra }));
      const page = () => label.textContent.match(/\d+ \/ \d+/)?.[0];
      const tf = () => {
        const s = target().style.transform;
        return { y: parseFloat(s.match(/translate3d\([^,]+,\s*(-?[\d.]+)px/)?.[1]), scale: parseFloat(s.match(/scale\(([\d.]+)\)/)?.[1]) };
      };
      const k = {};
      wheel(100);
      await sleep(700);
      k.wheel_down = page();
      wheel(-100);
      await sleep(700);
      k.wheel_up = page();
      key('ArrowRight');
      await sleep(700);
      k.arrow_right_ltr = page();
      key('ArrowLeft');
      await sleep(700);
      k.arrow_left_ltr = page();
      reader.toggleDirection(); // mangá: ← avança, → volta
      await sleep(500);
      key('ArrowRight');
      await sleep(700);
      k.arrow_right_rtl = page();
      key('ArrowLeft');
      await sleep(700);
      k.arrow_left_rtl = page();
      reader.toggleDirection();
      await sleep(500);
      key('End');
      await sleep(700);
      k.end = page();
      key('Home');
      await sleep(700);
      k.home = page();
      key('g');
      await sleep(300);
      document.getElementById('goto-input').value = '4';
      document.getElementById('goto-form').requestSubmit();
      await sleep(800);
      k.goto = page();
      await reader.seekPage(2);
      await sleep(800);
      for (let i = 0; i < 4; i += 1) key('+'); // 1,25 -> 2,0
      await sleep(400);
      k.zoom_scale = tf().scale;
      const y0 = tf().y;
      key('ArrowDown');
      await sleep(300);
      const y1 = tf().y;
      k.arrow_pans_when_zoomed = y1 < y0 && page() === '2 / 6';
      wheel(60);
      await sleep(300);
      k.wheel_pans_when_zoomed = tf().y < y1 && page() === '2 / 6';
      wheel(-120, { ctrlKey: true }); // Ctrl+roda para cima amplia
      await sleep(300);
      k.ctrl_wheel_zoomed_more = tf().scale > k.zoom_scale;
      key('0');
      await sleep(400);
      k.zoom_reset = tf().scale;
      results.steps.cbz_shortcuts = k;
      const want = {
        wheel_down: '3 / 6',
        wheel_up: '2 / 6',
        arrow_right_ltr: '3 / 6',
        arrow_left_ltr: '2 / 6',
        arrow_right_rtl: '1 / 6',
        arrow_left_rtl: '2 / 6',
        end: '6 / 6',
        home: '1 / 6',
        goto: '4 / 6',
        zoom_scale: 2,
        arrow_pans_when_zoomed: true,
        wheel_pans_when_zoomed: true,
        ctrl_wheel_zoomed_more: true,
        zoom_reset: 1,
      };
      const bad = Object.entries(want).filter(([name, v]) => k[name] !== v);
      if (bad.length || reader.getPrefs().direction !== 'ltr') {
        throw new Error(`CBZ atalhos/roda: ${JSON.stringify(bad)} | ${JSON.stringify(k)}`);
      }
    }

    // 4b. Quadrinhos: direção RTL, duas páginas, folha larga, zoom por toque duplo, sem margens
    {
      const reader = appState.activeReader;
      const stage = document.querySelector('.cbz-stage');
      const label = document.getElementById('reader-status-label');
      const target = () => document.querySelector('.cbz-target');
      const tapAt = (frac, count = 1) => {
        const r = stage.getBoundingClientRect();
        const opts = { bubbles: true, clientX: r.left + r.width * frac, clientY: r.top + r.height / 2, pointerId: 11 };
        for (let i = 0; i < count; i += 1) {
          stage.dispatchEvent(new PointerEvent('pointerdown', opts));
          stage.dispatchEvent(new PointerEvent('pointerup', opts));
        }
      };
      const c = {};

      // RTL: toque na direita VOLTA a página, na esquerda AVANÇA; o estado é salvo por livro.
      c.dir_default = reader.getPrefs().direction;
      reader.toggleDirection();
      await sleep(300);
      tapAt(0.9);
      await sleep(700);
      c.rtl_right_tap = label.textContent;
      tapAt(0.1);
      await sleep(700);
      c.rtl_left_tap = label.textContent;
      await sleep(800);
      c.rtl_saved = (await api.getReaderPrefs(appState.activeBook.book_id)).prefs.direction;
      reader.toggleDirection();
      await sleep(300);

      // Toque duplo: amplia (2,5x) e volta; não vira página.
      tapAt(0.5, 2);
      await sleep(500);
      c.zoom_after_double = target().style.transform;
      c.label_after_double = label.textContent;
      tapAt(0.5, 2);
      await sleep(500);
      c.zoom_after_second_double = target().style.transform;

      // Duas páginas: capa sozinha, par 2–3, folha larga (5) sozinha.
      reader.cycleSpreadMode();
      await sleep(2000); // o pareamento espera as dimensões de todas as páginas
      c.spread_mode = reader.getPrefs().spread_mode;
      await reader.seekPage(1);
      await sleep(600);
      const seq = [label.textContent];
      for (let i = 0; i < 4; i += 1) {
        await reader.next();
        await sleep(600);
        seq.push(label.textContent);
      }
      c.double_sequence = seq;
      await reader.seekPage(2);
      await sleep(600);
      const ltrAlts = [...target().querySelectorAll('img')].map((im) => im.alt);
      reader.toggleDirection();
      await sleep(700);
      const rtlAlts = [...target().querySelectorAll('img')].map((im) => im.alt);
      c.alts_ltr = ltrAlts;
      c.alts_rtl = rtlAlts;
      results.screenshots.push(await api.e2eCaptureScreenshot('13-cbz-double-rtl'));
      reader.toggleDirection();
      await sleep(300);

      // Automático em janela larga resolve para duas páginas; margens zero aumentam a página.
      reader.cycleSpreadMode();
      await sleep(500);
      c.auto_effective = reader.getEffectiveSpreadMode();
      c.auto_mode_pref = reader.getPrefs().spread_mode;
      c.stage_px = `${Math.round(stage.getBoundingClientRect().width)}x${Math.round(stage.getBoundingClientRect().height)}`;
      reader.cycleSpreadMode(); // volta para uma página
      await sleep(500);
      c.single_again = reader.getEffectiveSpreadMode();
      const hBefore = target().getBoundingClientRect().height;
      reader.toggleZeroMargin();
      await sleep(500);
      const hAfter = target().getBoundingClientRect().height;
      c.margin_growth = Math.round(hAfter - hBefore);
      reader.toggleZeroMargin();
      await sleep(300);
      await reader.seekPage(2);
      await sleep(1800);
      results.steps.cbz_comic_features = c;

      const problems = [];
      if (c.dir_default !== 'ltr') problems.push('direção padrão deveria ser ltr');
      if (c.rtl_right_tap !== 'Pág. 1 / 6' || c.rtl_left_tap !== 'Pág. 2 / 6') problems.push('zonas de toque em RTL');
      if (c.rtl_saved !== 'rtl') problems.push('direção não foi salva por livro');
      if (!c.zoom_after_double.includes('scale(2.5)') || !c.label_after_double.startsWith('Pág. 2 / 6')) problems.push('toque duplo não ampliou');
      if (!c.zoom_after_second_double.includes('scale(1)')) problems.push('segundo toque duplo não voltou');
      if (c.spread_mode !== 'double') problems.push('modo de duas páginas');
      if (seq.join('|') !== 'Pág. 1 / 6|Págs. 2–3 / 6|Pág. 4 / 6|Pág. 5 / 6|Pág. 6 / 6') problems.push('sequência de spreads');
      if (ltrAlts.join('|') !== 'Página 2 de 6|Página 3 de 6') problems.push('ordem LTR das páginas');
      if (rtlAlts.join('|') !== 'Página 3 de 6|Página 2 de 6') problems.push('ordem RTL das páginas');
      // Automático: duas páginas só se a janela for larga o bastante (aqui ~936x838, quase quadrada).
      const [sw, sh] = c.stage_px.split('x').map(Number);
      const expectedAuto = sw / sh >= 1.2 ? 'double' : 'single';
      if (c.auto_effective !== expectedAuto || c.single_again !== 'single') problems.push('modo automático');
      if (!(c.margin_growth > 10)) problems.push('margens zero não aumentaram a página');
      if (problems.length) throw new Error(`Quadrinhos: ${problems.join('; ')} :: ${JSON.stringify(c)}`);
    }

    await closeActiveReader();
    await sleep(400);

    results.steps.after_close_reader = {
      immersive: document.getElementById('reader-screen').classList.contains('immersive'),
      chrome_theme: document.getElementById('reader-screen').dataset.readerTheme || null,
    };
    if (results.steps.after_close_reader.immersive || results.steps.after_close_reader.chrome_theme) {
      throw new Error(`Leitor fechado não restaurou a interface: ${JSON.stringify(results.steps.after_close_reader)}`);
    }

    // 2b. Capas, nome de exibição e "Continuar lendo" (tudo pelo caminho real: servidor, cliente,
    // protocolo local e DOM).
    {
      const cv = {};
      const problems = [];
      const waitFor = async (pred, ms = 15000) => {
        const t0 = Date.now();
        while (Date.now() - t0 < ms) {
          if (pred()) return true;
          await sleep(150);
        }
        return false;
      };
      const byId = (id) => appState.books.find((b) => b.book_id === id);
      const cardIn = (sel, id) => document.querySelector(`${sel} .book-card[data-book-id="${id}"]`);

      // Capas embutidas geradas em segundo plano (CBZ: página 1; PDF: renderiza a página 1).
      cv.embedded_generated = await waitFor(
        () => Number(byId(cbzBook.book_id)?.cover_version) > 0 && Number(byId(pdfBook.book_id)?.cover_version) > 0,
      );
      if (!cv.embedded_generated) problems.push('capas embutidas não foram geradas');
      cv.epub_cover_version = Number(byId(epubBook.book_id)?.cover_version) || 0;

      // O protocolo local serve a capa como JPEG de verdade, pequena.
      const coverUrl = await api.ensureCover(cbzBook.book_id);
      const resp = await fetch(coverUrl);
      const blob = await resp.blob();
      cv.cbz_cover_http = { status: resp.status, type: blob.type, size: blob.size };
      if (resp.status !== 200 || blob.type !== 'image/jpeg' || blob.size < 100 || blob.size > 400000) {
        problems.push('capa do CBZ não foi servida como JPEG pequeno');
      }
      const bmp = await createImageBitmap(blob);
      cv.cbz_cover_px = `${bmp.width}x${bmp.height}`;
      if (bmp.width > 360 || bmp.height > 540) problems.push('capa maior que 360x540');

      // O card do PDF mostra a imagem (pasta do livro) e depois volta à raiz.
      navigateToFolder(pdfBook.folder || '');
      await sleep(200);
      const pdfCardSel = `.book-card[data-book-id="${pdfBook.book_id}"] img.book-cover-img`;
      cv.pdf_card_image = await waitFor(() => {
        const img = document.querySelector(pdfCardSel);
        return Boolean(img && img.complete && img.naturalWidth > 0);
      }, 8000);
      if (!cv.pdf_card_image) problems.push('card do PDF sem imagem de capa');
      results.screenshots.push(await api.e2eCaptureScreenshot('12-covers'));

      // Nome de exibição: aparece no card, o original fica no `title`, e limpar volta ao original.
      await api.setDisplayTitle(pdfBook.book_id, '  Meu   PDF  Teste ');
      const titleSel = `.book-card[data-book-id="${pdfBook.book_id}"] .book-title`;
      cv.title_shown = await waitFor(
        () => document.querySelector(titleSel)?.textContent === 'Meu PDF Teste',
        6000,
      );
      if (!cv.title_shown) problems.push('nome de exibição não apareceu no card');
      if (byId(pdfBook.book_id)?.title !== pdfBook.title) problems.push('título original foi alterado');
      await api.setDisplayTitle(pdfBook.book_id, null);
      cv.title_restored = await waitFor(
        () => document.querySelector(titleSel)?.textContent === pdfBook.title,
        6000,
      );
      if (!cv.title_restored) problems.push('nome original não voltou');
      navigateToFolder('');
      await sleep(200);

      // Imagem própria: vence a capa embutida; embutida nova é ignorada; restaurar regera.
      const png = document.createElement('canvas');
      png.width = 800;
      png.height = 1200;
      const pctx = png.getContext('2d');
      pctx.fillStyle = '#c0392b';
      pctx.fillRect(0, 0, 800, 1200);
      const pngBlob = await new Promise((r) => png.toBlob(r, 'image/png'));
      const customBytes = await resizeImageToJpeg(pngBlob);
      const before = Number(byId(cbzBook.book_id).cover_version);
      const custom = await api.setBookCover(cbzBook.book_id, 'custom', customBytes);
      cv.custom_cover = { applied: custom.applied, source: custom.cover_source, bumped: custom.cover_version > before };
      const ignored = await api.setBookCover(cbzBook.book_id, 'embedded', customBytes);
      cv.embedded_ignored = ignored.applied === false && ignored.cover_source === 'custom';
      if (!custom.applied || custom.cover_source !== 'custom' || !(custom.cover_version > before)) {
        problems.push('capa própria não foi aplicada');
      }
      if (!cv.embedded_ignored) problems.push('capa embutida sobrescreveu a do usuário');
      await api.removeBookCover(cbzBook.book_id);
      await refreshLibrary(false); // dispara a geração em segundo plano da capa original
      cv.original_regenerated = await waitFor(() => {
        const b = byId(cbzBook.book_id);
        return Number(b?.cover_version) > 0 && b?.cover_source === 'embedded';
      }, 15000);
      if (!cv.original_regenerated) problems.push('capa original não foi regerada após restaurar');

      // Tirar do Continuar lendo: some da seção mas continua na biblioteca; ler de novo traz de volta.
      const inContinue = () => Boolean(cardIn('#continue-reading-grid', epubBook.book_id));
      cv.continue_has_epub = await waitFor(inContinue, 5000);
      if (!cv.continue_has_epub) problems.push('livro lido não está em Continuar lendo');
      cardIn('#continue-reading-grid', epubBook.book_id)
        ?.querySelector('.book-continue-hide-btn')
        ?.click();
      cv.hidden_from_continue = await waitFor(() => !inContinue(), 6000);
      if (!cv.hidden_from_continue) problems.push('botão ✕ não tirou do Continuar lendo');
      cv.still_in_library = Boolean(byId(epubBook.book_id)) && byId(epubBook.book_id).position?.percent > 0;
      if (!cv.still_in_library) problems.push('livro sumiu da biblioteca ou perdeu o progresso');
      results.screenshots.push(await api.e2eCaptureScreenshot('13-continue-hidden'));
      await sleep(80);
      const pos = await api.getPosition(epubBook.book_id);
      await api.setPosition(epubBook.book_id, pos.locator, pos.percent, pos.snippet);
      cv.back_after_reading = await waitFor(inContinue, 8000);
      if (!cv.back_after_reading) problems.push('livro não voltou ao Continuar lendo depois de ler');

      // Só abrir o livro (sem virar página, sem salvar posição) também traz de volta.
      cardIn('#continue-reading-grid', epubBook.book_id)
        ?.querySelector('.book-continue-hide-btn')
        ?.click();
      cv.hidden_again = await waitFor(() => !inContinue(), 6000);
      if (!cv.hidden_again) problems.push('segundo ✕ não tirou do Continuar lendo');
      await openBookFlow(byId(epubBook.book_id) || epubBook);
      // O livro deixa de estar escondido assim que abre (conferido no estado, com o leitor aberto)...
      cv.unhidden_on_open = await waitFor(() => !byId(epubBook.book_id)?.hidden_continue_at_ms, 8000);
      document.getElementById('btn-reader-back')?.click();
      // ...e aparece em Continuar lendo ao voltar para a biblioteca.
      cv.back_after_open_only = await waitFor(inContinue, 8000);
      if (!cv.unhidden_on_open || !cv.back_after_open_only) {
        problems.push('só abrir o livro não o trouxe de volta ao Continuar lendo');
      }
      await sleep(300);

      // Modal "Editar livro": abre pelo botão ⋯, renomeia pelo campo, mostra a busca com o texto
      // já limpo e fecha com Esc (a busca em si depende da internet, então não é exigida aqui).
      navigateToFolder(pdfBook.folder || '');
      await sleep(250);
      document.querySelector(`.book-card[data-book-id="${pdfBook.book_id}"] .book-menu-btn`)?.click();
      await sleep(200);
      const modal = document.getElementById('book-menu-modal');
      cv.menu_open = Boolean(modal && !modal.classList.contains('hidden'));
      if (!cv.menu_open) problems.push('modal Editar livro não abriu');
      const nameInput = document.getElementById('book-menu-name');
      nameInput.value = 'Nome pelo modal';
      document.getElementById('book-menu-name-save').click();
      cv.menu_rename = await waitFor(
        () => document.querySelector(titleSel)?.textContent === 'Nome pelo modal',
        6000,
      );
      if (!cv.menu_rename) problems.push('renomear pelo modal não refletiu no card');
      document.getElementById('book-menu-web-search').click();
      await sleep(300);
      cv.web_query = document.getElementById('book-menu-web-query').value;
      if (cv.web_query !== 'Nome pelo modal') problems.push('busca web sem o texto do livro');
      results.screenshots.push(await api.e2eCaptureScreenshot('14-book-menu'));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
      await sleep(200);
      cv.menu_closed_by_esc = modal.classList.contains('hidden');
      if (!cv.menu_closed_by_esc) problems.push('Esc não fechou o modal');
      await api.setDisplayTitle(pdfBook.book_id, null);
      navigateToFolder('');
      await sleep(200);

      results.steps.covers_title_continue = cv;
      if (problems.length) throw new Error(`Capas/nome/continuar: ${problems.join('; ')} :: ${JSON.stringify(cv)}`);
    }

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

    {
      const stage = document.querySelector('.zoom-stage');
      const screenEl = document.getElementById('reader-screen');
      const label = document.getElementById('reader-status-label');
      const tapStage = (frac) => {
        const r = stage.getBoundingClientRect();
        const opts = { bubbles: true, clientX: r.left + r.width * frac, clientY: r.top + r.height / 2, pointerId: 9 };
        stage.dispatchEvent(new PointerEvent('pointerdown', opts));
        stage.dispatchEvent(new PointerEvent('pointerup', opts));
      };
      const t = { start: label.textContent };
      tapStage(0.5);
      await sleep(300);
      t.immersive_after_center = screenEl.classList.contains('immersive');
      tapStage(0.5);
      await sleep(300);
      t.immersive_after_second = screenEl.classList.contains('immersive');
      tapStage(0.9);
      await sleep(1200);
      t.after_right_tap = label.textContent;
      tapStage(0.1);
      await sleep(1200);
      t.after_left_tap = label.textContent;
      results.steps.pdf_tap_zones = t;
      if (!t.immersive_after_center || t.immersive_after_second) throw new Error(`PDF imersivo: ${JSON.stringify(t)}`);
      if (t.start !== 'Pág. 4 / 5' || t.after_right_tap !== 'Pág. 5 / 5' || t.after_left_tap !== 'Pág. 4 / 5') {
        throw new Error(`PDF zonas de toque: ${JSON.stringify(t)}`);
      }
    }

    // Atalhos de teclado e roda do mouse no PDF (estamos na pág. 4 de 5, sem zoom).
    {
      const stage = document.querySelector('.zoom-stage');
      const canvas = document.querySelector('.pdf-canvas');
      const wrap = document.querySelector('.zoom-target');
      const label = document.getElementById('reader-status-label');
      const key = (k, extra = {}) =>
        window.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...extra }));
      const wheel = (deltaY, extra = {}) =>
        stage.dispatchEvent(new WheelEvent('wheel', { deltaY, bubbles: true, cancelable: true, ...extra }));
      const page = () => label.textContent.match(/\d+ \/ \d+/)?.[0];
      const canvasW = () => parseFloat(canvas.style.width);
      const ty = () => new DOMMatrixReadOnly(getComputedStyle(wrap).transform).m42;
      const k = {};

      wheel(100);
      await sleep(1200);
      k.wheel_down_page = page();
      wheel(100); // já é a última: não pode passar do fim
      await sleep(500);
      k.wheel_down_at_end = page();
      wheel(-100);
      await sleep(1200);
      k.wheel_up_page = page();
      key('Home');
      await sleep(1200);
      k.home = page();
      key('End');
      await sleep(1200);
      k.end = page();
      key('ArrowUp');
      await sleep(1200);
      k.arrow_up_page = page(); // sem zoom a seta vira a página

      const w0 = canvasW();
      key('+');
      await sleep(900);
      k.zoom_in_grew = canvasW() > w0 * 1.15;
      const p0 = page();
      const y0 = ty();
      key('ArrowDown'); // ampliado: a seta move a página, não vira
      await sleep(300);
      k.arrow_pans_when_zoomed = ty() < y0 && page() === p0;
      const y1 = ty();
      await sleep(300);
      wheel(60); // ampliado: a roda rola a página
      await sleep(300);
      k.wheel_pans_when_zoomed = ty() < y1 && page() === p0;
      key('-');
      await sleep(900);
      wheel(120, { ctrlKey: true }); // Ctrl+roda para baixo reduz
      await sleep(900);
      key('0');
      await sleep(900);
      k.zoom_reset_width = Math.abs(canvasW() - w0) < 2;
      wheel(-4, { ctrlKey: true }); // pinça de trackpad: delta pequeno = zoom pequeno
      await sleep(900);
      k.pinch_small_step = canvasW() > w0 && canvasW() < w0 * 1.06;
      key('0');
      await sleep(900);

      key('g');
      await sleep(300);
      const gotoModal = document.getElementById('goto-modal');
      k.goto_opened = !gotoModal.classList.contains('hidden');
      document.getElementById('goto-input').value = '2';
      document.getElementById('goto-form').requestSubmit();
      await sleep(1200);
      k.goto_closed = gotoModal.classList.contains('hidden');
      k.goto_page = page();

      key('?');
      await sleep(300);
      const help = document.getElementById('shortcuts-modal');
      k.help_opened = !help.classList.contains('hidden');
      k.help_has_rows = document.querySelectorAll('#shortcuts-body .shortcuts-row').length >= 15;
      key('ArrowRight'); // com a ajuda aberta o livro não pode virar
      await sleep(500);
      k.page_unchanged_under_help = page() === k.goto_page;
      key('Escape');
      await sleep(300);
      k.help_closed_by_esc = help.classList.contains('hidden');
      k.reader_still_open = Boolean(appState.activeReader);

      results.steps.pdf_shortcuts = k;
      const expect = {
        wheel_down_page: '5 / 5',
        wheel_down_at_end: '5 / 5',
        wheel_up_page: '4 / 5',
        home: '1 / 5',
        end: '5 / 5',
        arrow_up_page: '4 / 5',
        zoom_in_grew: true,
        arrow_pans_when_zoomed: true,
        wheel_pans_when_zoomed: true,
        zoom_reset_width: true,
        pinch_small_step: true,
        goto_opened: true,
        goto_closed: true,
        goto_page: '2 / 5',
        help_opened: true,
        help_has_rows: true,
        page_unchanged_under_help: true,
        help_closed_by_esc: true,
        reader_still_open: true,
      };
      const bad = Object.entries(expect).filter(([name, want]) => k[name] !== want);
      if (bad.length) throw new Error(`PDF atalhos/roda: ${JSON.stringify(bad)} | ${JSON.stringify(k)}`);
    }

    await closeActiveReader();
    await sleep(500);

    // Screenshot da biblioteca atualizada com barras de progresso e "Continuar lendo"
    results.screenshots.push(await api.e2eCaptureScreenshot('07-library-updated'));

    // QR de pareamento: o SVG REAL do servidor, rasterizado em tamanhos de camera, tem de decodificar
    // (jsQR + canvas no webview) para o mesmo JSON e passar na validacao de pareamento.
    {
      const qrDto = await api.rotatePairingQr();
      const svgUrl = URL.createObjectURL(new Blob([qrDto.svg], { type: 'image/svg+xml' }));
      const img = new Image();
      await new Promise((resolve, reject) => {
        img.onload = resolve;
        img.onerror = () => reject(new Error('SVG do QR nao carregou'));
        img.src = svgUrl;
      });
      const canvas = document.createElement('canvas');
      const decoded = {};
      for (const side of [600, 360, 240]) {
        const frame = grabFrame(img, side, side, canvas);
        decoded[side] = decodeQrFromImageData(frame);
      }
      URL.revokeObjectURL(svgUrl);
      results.steps.qr_decode_roundtrip = {
        matches: Object.fromEntries(Object.entries(decoded).map(([k, v]) => [k, v === qrDto.qr_json])),
        pairing_payload_accepted: parsePairingPayload(decoded[360]) === qrDto.qr_json,
        foreign_qr_rejected: parsePairingPayload('https://example.com') === null,
      };
      const r = results.steps.qr_decode_roundtrip;
      if (!Object.values(r.matches).every(Boolean) || !r.pairing_payload_accepted || !r.foreign_qr_rejected) {
        throw new Error(`QR de pareamento nao fez o roundtrip: ${JSON.stringify(r)}`);
      }
    }

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
