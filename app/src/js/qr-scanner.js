// Leitura do QR Code de pareamento pela camera (Android). Usa getUserMedia no WebView (o Tauri/wry
// pede a permissao CAMERA do Android sozinho) e o decodificador jsQR vendorizado, sem plugin nativo
// e sem depender dos Servicos do Google. A logica pura (validar o payload, mensagens de erro) fica
// em state.js.

import '../vendor/jsqr/jsQR.js';

/**
 * Teto de pixels do quadro enviado ao jsQR. O QR de pareamento e denso (dezenas de modulos de ~3-5 px
 * na camera): reduzir o quadro para 640 px o deixava com ~2 px por modulo e ele nao lia. Acima de
 * ~1,3 MP a leitura so fica lenta.
 */
const MAX_DECODE_PIXELS = 1_300_000;
/** Pausa entre o fim de uma tentativa e o inicio da proxima (ms). */
const SCAN_INTERVAL_MS = 60;

/**
 * Decodifica um QR Code de um quadro de imagem.
 * @param {{ data: Uint8ClampedArray, width: number, height: number }} imageData
 * @param {Function} [jsQR] decodificador (injetavel para teste)
 * @returns {string|null} texto do QR ou null se nao achou
 */
export function decodeQrFromImageData(imageData, jsQR = globalThis.jsQR) {
  const hit = jsQR(imageData.data, imageData.width, imageData.height, {
    // O QR de pareamento e escuro sobre claro; testar invertido dobraria o custo por quadro.
    inversionAttempts: 'dontInvert',
  });
  return hit && typeof hit.data === 'string' && hit.data !== '' ? hit.data : null;
}

/**
 * Desenha `source` (video ou imagem) reduzido no canvas e devolve o ImageData.
 * @param {HTMLVideoElement|HTMLImageElement} source
 * @param {number} width largura natural da fonte
 * @param {number} height altura natural da fonte
 * @param {HTMLCanvasElement} canvas
 */
export function grabFrame(source, width, height, canvas) {
  const scale = Math.min(1, Math.sqrt(MAX_DECODE_PIXELS / (width * height)));
  const w = Math.max(1, Math.round(width * scale));
  const h = Math.max(1, Math.round(height * scale));
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  ctx.drawImage(source, 0, 0, w, h);
  return ctx.getImageData(0, 0, w, h);
}

/**
 * Abre a camera traseira em `video` e le quadros ate `accept` aceitar um texto.
 * Sempre solta a camera ao terminar (sucesso, erro ou cancelamento).
 * @param {object} opts
 * @param {HTMLVideoElement} opts.video
 * @param {HTMLCanvasElement} opts.canvas
 * @param {(text: string) => string|null} opts.accept devolve o valor aceito ou null para ignorar
 * @param {(text: string) => void} [opts.onIgnored] chamado quando um QR foi lido mas rejeitado
 * @param {AbortSignal} opts.signal cancela a leitura (rejeita com AbortError)
 * @returns {Promise<string>} valor devolvido por `accept`
 */
export async function scanQrFromCamera({ video, canvas, accept, onIgnored, signal }) {
  if (!navigator.mediaDevices?.getUserMedia) {
    throw Object.assign(new Error('getUserMedia indisponivel'), { name: 'NotFoundError' });
  }
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: false,
    video: { facingMode: { ideal: 'environment' }, width: { ideal: 1920 }, height: { ideal: 1080 } },
  });
  try {
    if (signal.aborted) throw new DOMException('cancelado', 'AbortError');
    video.srcObject = stream;
    video.setAttribute('playsinline', '');
    video.muted = true;
    await video.play();
    let lastIgnored = '';
    let dbgFrames = 0;
    let dbgMs = 0;
    console.info(`[qr] video ${video.videoWidth}x${video.videoHeight} BarcodeDetector=${'BarcodeDetector' in window}`);
    return await new Promise((resolve, reject) => {
      let timer = null;
      const stop = () => {
        if (timer) clearTimeout(timer);
        signal.removeEventListener('abort', onAbort);
      };
      const onAbort = () => {
        stop();
        reject(new DOMException('cancelado', 'AbortError'));
      };
      signal.addEventListener('abort', onAbort);
      const tick = () => {
        try {
          if (video.videoWidth > 0 && video.videoHeight > 0) {
            const t0 = performance.now();
            const frame = grabFrame(video, video.videoWidth, video.videoHeight, canvas);
            const text = decodeQrFromImageData(frame);
            dbgMs += performance.now() - t0;
            if (++dbgFrames % 10 === 0) console.info(`[qr] ${frame.width}x${frame.height} media ${Math.round(dbgMs / dbgFrames)} ms/quadro (${dbgFrames} quadros)`);
            if (text) {
              const accepted = accept(text);
              if (accepted) {
                stop();
                resolve(accepted);
                return;
              }
              if (text !== lastIgnored) {
                lastIgnored = text;
                onIgnored?.(text);
              }
            }
          }
        } catch (err) {
          stop();
          reject(err);
          return;
        }
        timer = setTimeout(tick, SCAN_INTERVAL_MS);
      };
      tick();
    });
  } finally {
    for (const track of stream.getTracks()) track.stop();
    video.srcObject = null;
  }
}
