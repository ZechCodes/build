import { RECENT_AFTER_MS } from "./inboxOrdering.js";

const MAX_TIMEOUT_MS = 2 ** 31 - 1;

function nextCutoff(entries, nowMs) {
  return entries.reduce((next, entry) => {
    const cutoff = entry.lastActivityMs + RECENT_AFTER_MS;
    return !entry.dismissed && Number.isFinite(entry.lastActivityMs) && cutoff > nowMs
      ? Math.min(next, cutoff) : next;
  }, Infinity);
}

/** Age held rows at their next Recent boundary, without reading the wire. A
 * visible wake rechecks the cache projection after a suspended page's timers. */
export function createInboxClock(redraw) {
  let timer = null;
  let disposed = false;

  const clear = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const wake = () => {
    if (!disposed && !document.hidden) redraw();
  };
  document.addEventListener("visibilitychange", wake);
  window.addEventListener("pageshow", wake);

  return {
    update(entries, nowMs = Date.now()) {
      clear();
      if (disposed) return;
      const cutoff = nextCutoff(entries, nowMs);
      if (cutoff === Infinity) return;
      timer = setTimeout(redraw, Math.min(MAX_TIMEOUT_MS, cutoff - nowMs));
    },
    dispose() {
      disposed = true;
      clear();
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("pageshow", wake);
    },
  };
}
