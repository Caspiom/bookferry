// Camada de comunicação com o backend Tauri (comandos, eventos ClientEvent e protocolo de arquivos).

let protocolBaseUrl = 'bookferry://localhost';

/**
 * Define a URL base do protocolo customizado informada pelo comando `status`.
 * Desktop Linux: `bookferry://localhost`
 * Android: `http://bookferry.localhost`
 * @param {string} baseUrl
 */
export function setProtocolBaseUrl(baseUrl) {
  if (typeof baseUrl === 'string' && baseUrl.trim()) {
    protocolBaseUrl = baseUrl.trim().replace(/\/+$/, '');
  }
}

/**
 * Retorna a URL do protocolo customizado para um arquivo de livro baixado (EPUB ou PDF com suporte a Range).
 * @param {string} bookId
 * @returns {string}
 */
export function getBookFileUrl(bookId) {
  return `${protocolBaseUrl}/books/${encodeURIComponent(bookId)}`;
}

/**
 * Retorna a URL do protocolo customizado para a página `pageIndex` (0-based) de um CBZ.
 * @param {string} bookId
 * @param {number} pageIndex
 * @returns {string}
 */
export function getCbzPageUrl(bookId, pageIndex) {
  return `${protocolBaseUrl}/cbz/${encodeURIComponent(bookId)}/page/${Number(pageIndex) || 0}`;
}

function getTauriCore() {
  if (typeof window !== 'undefined' && window.__TAURI__ && window.__TAURI__.core) {
    return window.__TAURI__.core;
  }
  throw new Error('API Tauri indisponível neste ambiente.');
}

/**
 * Invoca um comando Tauri.
 * @param {string} cmd
 * @param {object} [args]
 */
export async function invokeCommand(cmd, args = {}) {
  const core = getTauriCore();
  return core.invoke(cmd, args);
}

/**
 * Registra listener para `ClientEvent` emitido pelo Rust no canal `bookferry-event`.
 * @param {(eventPayload: object) => void} handler
 * @returns {Promise<() => void>} Função unlisten
 */
export async function listenClientEvents(handler) {
  if (typeof window !== 'undefined' && window.__TAURI__ && window.__TAURI__.event) {
    return window.__TAURI__.event.listen('bookferry-event', (event) => {
      if (event && event.payload) {
        handler(event.payload);
      }
    });
  }
  return () => {};
}

// Comandos tipados do backend Rust
export const api = {
  status: () => invokeCommand('status'),
  pair: (qrJson, deviceName = null) => invokeCommand('pair', { qrJson, deviceName }),
  unpair: () => invokeCommand('unpair'),
  library: () => invokeCommand('library'),
  rescan: () => invokeCommand('rescan'),
  downloadBook: (bookId) => invokeCommand('download_book', { bookId }),
  cancelDownload: (bookId) => invokeCommand('cancel_download', { bookId }),
  removeDownload: (bookId) => invokeCommand('remove_download', { bookId }),
  setPosition: (bookId, locator, percent, snippet = null) =>
    invokeCommand('set_position', { bookId, locator, percent, snippet }),
  getPosition: (bookId) => invokeCommand('get_position', { bookId }),
  decideBookResume: (bookId) => invokeCommand('decide_book_resume', { bookId }),
  onResume: () => invokeCommand('on_resume'),
  cbzInfo: (bookId) => invokeCommand('cbz_info', { bookId }),
  saveSettings: ({ defaultFontSize, theme, clientOnlyMode }) =>
    invokeCommand('save_settings', { defaultFontSize, theme, clientOnlyMode }),
  configureServerVault: (vaultDir) => invokeCommand('configure_server_vault', { vaultDir }),
  rotatePairingQr: () => invokeCommand('rotate_pairing_qr'),
  listPairedDevices: () => invokeCommand('list_paired_devices'),
  revokePairedDevice: (deviceId) => invokeCommand('revoke_paired_device', { deviceId }),
  // Preferências de leitura (por livro, padrão global, presets e linha de progresso)
  getReaderPrefs: (bookId) => invokeCommand('get_reader_prefs', { bookId }),
  saveReaderPrefs: (bookId, prefs) => invokeCommand('save_reader_prefs', { bookId, prefs }),
  resetReaderPrefs: (bookId) => invokeCommand('reset_reader_prefs', { bookId }),
  setDefaultReaderPrefs: (bookId, prefs) =>
    invokeCommand('set_default_reader_prefs', { bookId, prefs }),
  saveReaderPreset: (bookId, name, prefs) =>
    invokeCommand('save_reader_preset', { bookId, name, prefs }),
  deleteReaderPreset: (bookId, name) => invokeCommand('delete_reader_preset', { bookId, name }),
  saveProgressPrefs: (bookId, progress) =>
    invokeCommand('save_progress_prefs', { bookId, progress }),
  setFullscreen: (fullscreen) => invokeCommand('set_fullscreen', { fullscreen }),
  isFullscreen: () => invokeCommand('is_fullscreen'),
  // Comandos E2E (disponíveis apenas quando compilado com BOOKFERRY_E2E=1)
  e2ePairSecondClient: (deviceName) => invokeCommand('e2e_pair_second_client', { deviceName }),
  e2eWaitPeerPosition: (bookId, minPercent, timeoutMs, expectedLocatorPrefix = null) =>
    invokeCommand('e2e_wait_peer_position', {
      bookId,
      minPercent,
      timeoutMs,
      expectedLocatorPrefix,
    }),
  e2ePeerSetPosition: (bookId, locator, percent, snippet = null) =>
    invokeCommand('e2e_peer_set_position', { bookId, locator, percent, snippet }),
  e2eCaptureScreenshot: (name) => invokeCommand('e2e_capture_screenshot', { name }),
  e2eFinish: (reportObj) =>
    invokeCommand('e2e_finish', {
      reportJson: typeof reportObj === 'string' ? reportObj : JSON.stringify(reportObj),
    }),
};
