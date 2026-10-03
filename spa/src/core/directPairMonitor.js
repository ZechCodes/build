/** Observe a relayed path at a bounded cadence, including late candidate
 *  resolution. A check never overlaps another, and closing prevents an awaited
 *  check from scheduling more work. The caller alone decides when to restart. */
export function createDirectPairMonitor({
  check,
  canCheck,
  initialDelayMs = 20000,
  intervalMs = 5000,
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (timer) => clearTimeout(timer),
}) {
  let timer = null;
  let running = false;
  let closed = false;
  const schedule = (delayMs) => {
    if (closed || running || !canCheck()) return;
    clearTimer(timer);
    timer = setTimer(sample, delayMs);
  };
  const sample = async () => {
    timer = null;
    running = true;
    try {
      await check();
    } catch {
      // An optional upgrade never takes away the working fallback. The check
      // records negotiation failures; timer callbacks must not reject globally.
    } finally {
      running = false;
      schedule(intervalMs);
    }
  };
  return {
    start: () => schedule(initialDelayMs),
    close() {
      closed = true;
      clearTimer(timer);
      timer = null;
    },
  };
}
