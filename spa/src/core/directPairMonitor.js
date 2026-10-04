/** Viability stays observable before an upgrade, at an increasingly sparse
 *  cadence. Once the peer link spends its one attempt, observe native ICE for
 *  two more minutes: enough for late nomination without lifelong stats work. */
export function createDirectPairMonitor({
  check,
  canCheck,
  hasAttempted = () => false,
  initialDelayMs = 20000,
  intervalMs = 5000,
  maxIntervalMs = 60000,
  observationWindowMs = 120000,
  now = () => Date.now(),
  setTimer = (fn, ms) => setTimeout(fn, ms),
  clearTimer = (timer) => clearTimeout(timer),
}) {
  let timer = null;
  let running = false;
  let closed = false;
  let nextIntervalMs = intervalMs;
  let observationDeadline = null;
  let requestedStartAt = null;
  const schedule = (delayMs) => {
    const remainingMs = observationDeadline === null ? Infinity : observationDeadline - now();
    if (closed || running || !canCheck() || remainingMs <= 0) return;
    clearTimer(timer);
    timer = setTimer(sample, Math.min(delayMs, remainingMs));
  };
  const scheduleNextSample = () => {
    if (requestedStartAt !== null) {
      const delayMs = Math.max(0, requestedStartAt - now());
      requestedStartAt = null;
      schedule(delayMs);
      return;
    }
    schedule(nextIntervalMs);
    nextIntervalMs = Math.min(maxIntervalMs, nextIntervalMs * 2);
  };
  const sample = async () => {
    timer = null;
    if (closed || !canCheck() || (observationDeadline !== null && now() > observationDeadline)) return;
    running = true;
    try {
      await check();
    } catch {
      // The check records optional negotiation failures. Timer callbacks must
      // not reject globally and cannot take away the working fallback.
    } finally {
      running = false;
      if (hasAttempted() && observationDeadline === null) {
        observationDeadline = now() + observationWindowMs;
        nextIntervalMs = intervalMs;
      }
      scheduleNextSample();
    }
  };
  return {
    start() {
      if (!hasAttempted()) nextIntervalMs = intervalMs;
      // Recovery can finish while the previous getStats is still awaited.
      // Keep its settling deadline until that sample returns.
      requestedStartAt = now() + initialDelayMs;
      if (running) return;
      requestedStartAt = null;
      schedule(initialDelayMs);
    },
    close() {
      closed = true;
      clearTimer(timer);
      timer = null;
    },
  };
}
