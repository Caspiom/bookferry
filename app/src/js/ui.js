// Componentes de interface (toasts, modais de retomada/download, configurações e pareamento).

import { api } from './api.js';
import { formatPercent } from './state.js';

let toastTimeoutId = null;
let qrCountdownInterval = null;
const knownDeviceNames = new Map();

export function registerKnownDeviceName(deviceId, name) {
  if (deviceId && name) {
    knownDeviceNames.set(deviceId, name);
  }
}

export function resolveDeviceLabel(posOrDeviceId) {
  if (!posOrDeviceId) return 'outro aparelho';
  if (typeof posOrDeviceId === 'object') {
    if (posOrDeviceId.device_name) return posOrDeviceId.device_name;
    const id = posOrDeviceId.device_id;
    if (id && knownDeviceNames.has(id)) return knownDeviceNames.get(id);
    if (id) return `aparelho ${String(id).slice(0, 8)}`;
    return 'outro aparelho';
  }
  const id = String(posOrDeviceId);
  return knownDeviceNames.get(id) || `aparelho ${id.slice(0, 8)}`;
}

/**
 * Exibe um toast discreto ou interativo (ex.: "Ir para a posição de <device>?").
 * @param {string} message
 * @param {object} [options]
 * @param {string} [options.actionLabel]
 * @param {() => void} [options.onAction]
 * @param {number} [options.durationMs]
 */
export function showToast(message, options = {}) {
  const container = document.getElementById('toast-container');
  if (!container) return;

  if (toastTimeoutId) {
    clearTimeout(toastTimeoutId);
    toastTimeoutId = null;
  }

  container.innerHTML = '';
  const toastEl = document.createElement('div');
  toastEl.className = 'toast-item';
  toastEl.setAttribute('role', 'status');
  toastEl.setAttribute('aria-live', 'polite');

  const textSpan = document.createElement('span');
  textSpan.className = 'toast-text';
  textSpan.textContent = message;
  toastEl.appendChild(textSpan);

  if (options.actionLabel && typeof options.onAction === 'function') {
    const actionsDiv = document.createElement('div');
    actionsDiv.className = 'toast-actions';

    const actionBtn = document.createElement('button');
    actionBtn.type = 'button';
    actionBtn.className = 'btn btn-primary btn-sm';
    actionBtn.id = 'toast-action-btn';
    actionBtn.setAttribute('aria-label', options.actionLabel);
    actionBtn.textContent = options.actionLabel;
    actionBtn.addEventListener('click', () => {
      hideToast();
      options.onAction();
    });

    const dismissBtn = document.createElement('button');
    dismissBtn.type = 'button';
    dismissBtn.className = 'btn btn-ghost btn-sm';
    dismissBtn.id = 'toast-dismiss-btn';
    dismissBtn.setAttribute('aria-label', 'Dispensar aviso');
    dismissBtn.textContent = 'Agora não';
    dismissBtn.addEventListener('click', () => hideToast());

    actionsDiv.appendChild(actionBtn);
    actionsDiv.appendChild(dismissBtn);
    toastEl.appendChild(actionsDiv);
  }

  container.appendChild(toastEl);
  container.classList.remove('hidden');

  const duration = options.durationMs ?? (options.actionLabel ? 10000 : 4000);
  if (duration > 0) {
    toastTimeoutId = setTimeout(() => {
      hideToast();
    }, duration);
  }
}

export function hideToast() {
  const container = document.getElementById('toast-container');
  if (!container) return;
  if (toastTimeoutId) {
    clearTimeout(toastTimeoutId);
    toastTimeoutId = null;
  }
  container.classList.add('hidden');
  container.innerHTML = '';
}

/**
 * Exibe o modal de conflito de retomada (`ResumeDecision::Ask`):
 * "Continuar de onde parou no <device>? (p. X%)" com "Continuar lá" e "Ficar aqui".
 * @param {object} remotePos
 * @param {object|null} localPos
 * @returns {Promise<'remote'|'local'>}
 */
export function askResumeModal(remotePos, localPos) {
  return new Promise((resolve) => {
    const modal = document.getElementById('resume-modal');
    const msgEl = document.getElementById('resume-modal-message');
    const detailEl = document.getElementById('resume-modal-detail');
    const btnRemote = document.getElementById('resume-btn-remote');
    const btnLocal = document.getElementById('resume-btn-local');

    if (!modal || !msgEl || !btnRemote || !btnLocal) {
      resolve('remote');
      return;
    }

    const deviceName = resolveDeviceLabel(remotePos);
    const remotePct = formatPercent(remotePos?.percent ?? 0);
    const localPct = formatPercent(localPos?.percent ?? 0);

    msgEl.textContent = `Continuar de onde parou em ${deviceName}? (p. ${remotePct})`;
    if (detailEl) {
      const snippet = remotePos?.snippet ? ` — "${remotePos.snippet}"` : '';
      detailEl.textContent = `Posição remota: ${remotePct}${snippet} · Posição neste aparelho: ${localPct}`;
    }

    function cleanup(choice) {
      modal.classList.add('hidden');
      btnRemote.removeEventListener('click', onRemote);
      btnLocal.removeEventListener('click', onLocal);
      resolve(choice);
    }

    function onRemote() {
      cleanup('remote');
    }
    function onLocal() {
      cleanup('local');
    }

    btnRemote.addEventListener('click', onRemote);
    btnLocal.addEventListener('click', onLocal);
    modal.classList.remove('hidden');
    btnRemote.focus();
  });
}

/**
 * Controlador visual do modal de progresso de download.
 */
export const downloadModal = {
  show(bookTitle, onCancel) {
    const modal = document.getElementById('download-modal');
    const titleEl = document.getElementById('download-modal-title');
    const barEl = document.getElementById('download-progress-bar');
    const labelEl = document.getElementById('download-progress-label');
    const cancelBtn = document.getElementById('download-cancel-btn');
    if (!modal) return;

    if (titleEl) titleEl.textContent = `Baixando "${bookTitle}"...`;
    if (barEl) barEl.style.width = '0%';
    if (labelEl) labelEl.textContent = 'Iniciando transferência segura...';
    if (cancelBtn) {
      cancelBtn.onclick = () => {
        if (typeof onCancel === 'function') onCancel();
      };
    }
    modal.classList.remove('hidden');
  },

  update(downloadedBytes, totalBytes, formatBytesFn) {
    const barEl = document.getElementById('download-progress-bar');
    const labelEl = document.getElementById('download-progress-label');
    const pct = totalBytes > 0 ? Math.min(100, Math.round((downloadedBytes / totalBytes) * 100)) : 0;
    if (barEl) barEl.style.width = `${pct}%`;
    if (labelEl) {
      labelEl.textContent = `${formatBytesFn(downloadedBytes)} de ${formatBytesFn(totalBytes)} (${pct}%)`;
    }
  },

  hide() {
    const modal = document.getElementById('download-modal');
    if (modal) modal.classList.add('hidden');
  },
};

/**
 * Aplica o tema escolhido ('auto', 'light', 'dark', 'sepia') no documento.
 * @param {string} theme
 */
export function applyTheme(theme) {
  const root = document.documentElement;
  if (!theme || theme === 'auto') {
    root.removeAttribute('data-theme');
  } else {
    root.setAttribute('data-theme', theme);
  }
}

/**
 * Atualiza a contagem regressiva do QR de pareamento do servidor desktop.
 * @param {number} expiresAtMs
 */
function startQrCountdown(expiresAtMs) {
  if (qrCountdownInterval) {
    clearInterval(qrCountdownInterval);
    qrCountdownInterval = null;
  }

  const timerEl = document.getElementById('qr-countdown-label');
  if (!timerEl) return;

  function tick() {
    const remainingMs = Math.max(0, (Number(expiresAtMs) || 0) - Date.now());
    const totalSec = Math.ceil(remainingMs / 1000);
    const min = Math.floor(totalSec / 60);
    const sec = totalSec % 60;
    if (totalSec <= 0) {
      timerEl.textContent = 'Segredo expirado — clique em "Gerar novo"';
      timerEl.classList.add('text-danger');
    } else {
      timerEl.textContent = `Expira em ${String(min).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
      timerEl.classList.remove('text-danger');
    }
  }

  tick();
  qrCountdownInterval = setInterval(tick, 1000);
}

/**
 * Atualiza apenas os campos do QR Code (SVG, JSON e timer).
 * @param {object|null} qrDto
 */
export function updatePairingQrDisplay(qrDto) {
  const qrBox = document.getElementById('server-qr-svg-box');
  const qrJsonInput = document.getElementById('server-qr-json');
  if (!qrDto) return;

  if (qrBox) {
    qrBox.innerHTML = qrDto.svg || '';
  }
  if (qrJsonInput) {
    qrJsonInput.value = qrDto.qr_json || '';
  }
  if (qrDto.expires_at_ms) {
    startQrCountdown(qrDto.expires_at_ms);
  }
}

/**
 * Renderiza o bloco do servidor embutido (QR SVG, JSON copiável, contagem regressiva e lista de aparelhos pareados).
 * @param {object|null} embeddedServer
 */
export async function renderDesktopServerPanel(embeddedServer) {
  const serverSection = document.getElementById('settings-desktop-server');
  if (!serverSection) return;

  const qrBox = document.getElementById('server-qr-svg-box');
  const qrJsonInput = document.getElementById('server-qr-json');
  const vaultInput = document.getElementById('settings-vault-input');
  const serverStateBadge = document.getElementById('server-running-badge');

  if (!embeddedServer || !embeddedServer.running) {
    if (serverStateBadge) {
      serverStateBadge.textContent = 'Parado (defina a pasta de livros)';
      serverStateBadge.className = 'badge badge-offline';
    }
    if (qrBox) {
      qrBox.innerHTML =
        '<p class="muted-note">Configure uma pasta de livros válida acima para iniciar o servidor e gerar o QR de pareamento.</p>';
    }
    if (qrJsonInput) qrJsonInput.value = '';
    return;
  }

  if (vaultInput && embeddedServer.vault_dir) {
    vaultInput.value = embeddedServer.vault_dir;
  }
  if (serverStateBadge) {
    serverStateBadge.textContent = `Ativo na porta ${embeddedServer.port}`;
    serverStateBadge.className = 'badge badge-online';
  }

  if (embeddedServer.pairing_qr) {
    updatePairingQrDisplay(embeddedServer.pairing_qr);
  }

  await refreshPairedDevicesList();
}

/**
 * Carrega e exibe a lista de dispositivos pareados no servidor desktop, com botão "Revogar".
 */
export async function refreshPairedDevicesList() {
  const listEl = document.getElementById('paired-devices-list');
  if (!listEl) return;

  try {
    const devices = await api.listPairedDevices();
    listEl.innerHTML = '';
    const activeDevices = Array.isArray(devices)
      ? devices.filter((d) => !d.revoked)
      : [];

    for (const dev of activeDevices) {
      registerKnownDeviceName(dev.id, dev.name);
    }

    if (activeDevices.length === 0) {
      listEl.innerHTML = '<li class="muted-note">Nenhum dispositivo pareado.</li>';
      return;
    }

    for (const dev of activeDevices) {
      const li = document.createElement('li');
      li.className = 'device-item';

      const infoDiv = document.createElement('div');
      infoDiv.className = 'device-info';
      const nameStrong = document.createElement('strong');
      nameStrong.textContent = `${dev.name || dev.id}${dev.is_self ? ' (Este PC)' : ''}`;
      const metaSpan = document.createElement('span');
      metaSpan.className = 'device-meta';
      const lastSeen = dev.last_seen_at_ms
        ? new Date(dev.last_seen_at_ms).toLocaleString('pt-BR')
        : 'nunca';
      metaSpan.textContent = `ID: ${dev.id.slice(0, 8)}… · Último acesso: ${lastSeen}`;
      infoDiv.appendChild(nameStrong);
      infoDiv.appendChild(metaSpan);
      li.appendChild(infoDiv);

      if (!dev.is_self) {
        const revokeBtn = document.createElement('button');
        revokeBtn.type = 'button';
        revokeBtn.className = 'btn btn-danger btn-sm';
        revokeBtn.setAttribute('aria-label', `Revogar dispositivo ${dev.name}`);
        revokeBtn.textContent = 'Revogar';
        revokeBtn.addEventListener('click', async () => {
          try {
            revokeBtn.disabled = true;
            await api.revokePairedDevice(dev.id);
            showToast(`Dispositivo "${dev.name}" revogado.`);
            await refreshPairedDevicesList();
          } catch (err) {
            showToast(`Erro ao revogar: ${err}`);
            revokeBtn.disabled = false;
          }
        });
        li.appendChild(revokeBtn);
      }

      listEl.appendChild(li);
    }
  } catch (err) {
    listEl.innerHTML = `<li class="muted-note">Não foi possível listar dispositivos (${err}).</li>`;
  }
}
