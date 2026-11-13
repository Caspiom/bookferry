// Utilitários de gestos com DOM (a lógica pura fica em state.js).

export const DOUBLE_TAP_MS = 250;

/**
 * Distingue toque simples de toque duplo. O toque simples só dispara depois de `delayMs` sem um
 * segundo toque próximo; se vier o segundo, dispara apenas `onDouble` (nunca vira a página).
 * Com `delayMs = 0` o toque simples dispara na hora e não há toque duplo.
 * @param {{ delayMs?: number, slopPx?: number,
 *   onSingle: (pt: {x:number,y:number}) => void, onDouble?: (pt: {x:number,y:number}) => void }} opts
 */
export function createTapDispatcher({ delayMs = DOUBLE_TAP_MS, slopPx = 40, onSingle, onDouble }) {
  let pending = null;
  let timer = null;

  function clear() {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    pending = null;
  }

  function tap(x, y) {
    const pt = { x, y };
    if (delayMs <= 0) {
      onSingle(pt);
      return;
    }
    if (pending && Math.hypot(pending.x - x, pending.y - y) <= slopPx) {
      // Segundo toque: nunca dispara o toque simples (sem `onDouble` ele é apenas engolido,
      // evitando virar duas páginas num duplo clique para selecionar uma palavra).
      clear();
      if (onDouble) onDouble(pt);
      return;
    }
    clear();
    pending = pt;
    timer = setTimeout(() => {
      const p = pending;
      timer = null;
      pending = null;
      if (p) onSingle(p);
    }, delayMs);
  }

  return { tap, cancel: clear, isPending: () => pending !== null };
}
