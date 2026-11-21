import { httpFailure } from "../core/httpEndpoint.js";

const RETRY_DELAYS = [250, 500, 1000, 2000, 4000, 8000];
const PROBE_INTERVAL = 5000;
const CALL_TIMEOUT = 12000;
const ignore = () => {};

export function createWebSocketBinding({ owner, hasPanes, onReady, onLoss, onPush }) {
  let held;
  let ended = false;
  let failure;
  let retryTimer;
  let retries = 0;
  let operations = 0;

  function release(attempt) {
    if (held !== attempt) return false;
    held = null;
    clearTimeout(attempt.probe);
    attempt.stopPush?.();
    attempt.stopClose?.();
    attempt.abort.abort();
    attempt.carrier?.close();
    return true;
  }

  function failed(attempt, error) {
    if (!release(attempt) || ended) return;
    failure = error;
    onLoss(error);
    if (error.retryable && owner.isCurrent() && hasPanes()) scheduleRetry();
  }

  function scheduleRetry() {
    if (retryTimer !== undefined) return;
    const delay = RETRY_DELAYS[Math.min(retries++, RETRY_DELAYS.length - 1)];
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      if (hasPanes()) void ensure().catch(ignore);
    }, Math.round(delay * (0.8 + Math.random() * 0.2)));
  }

  function probeLater(attempt, misses = 0) {
    if (held !== attempt) return;
    attempt.probe = setTimeout(async () => {
      try {
        await attempt.carrier.call("ping", {}, { timeoutMs: 3000 });
        probeLater(attempt);
      } catch (error) {
        if (held !== attempt) return;
        if (!["busy", "timeout"].includes(error.error_code)) failed(attempt, error);
        else if (misses === 0) probeLater(attempt, 1);
        else failed(attempt, Object.assign(error, { retryable: true }));
      }
    }, PROBE_INTERVAL);
  }

  function accept(attempt, carrier) {
    if (held !== attempt || !owner.isCurrent()) {
      carrier.close();
      throw httpFailure("scope_ended");
    }
    attempt.carrier = carrier;
    attempt.stopPush = carrier.onPush((frame) => {
      if (held === attempt && owner.isCurrent()) onPush(frame);
    });
    attempt.stopClose = carrier.onClose((error) => failed(attempt, error));
    if (held !== attempt) throw failure;
    failure = undefined;
    retries = 0;
    probeLater(attempt);
    onReady(carrier);
    return carrier;
  }

  function ensure({ manual = false } = {}) {
    if (ended || !owner.isCurrent()) return Promise.reject(httpFailure("scope_ended"));
    if (held) return held.ready;
    if (!manual && failure && (!failure.retryable || retryTimer !== undefined)) return Promise.reject(failure);
    clearTimeout(retryTimer);
    retryTimer = undefined;
    const attempt = { abort: new AbortController() };
    held = attempt;
    attempt.ready = owner.open(attempt.abort.signal).then((carrier) => accept(attempt, carrier)).catch((error) => {
      failed(attempt, error);
      throw error;
    });
    return attempt.ready;
  }

  function idle() {
    if (operations || hasPanes()) return;
    clearTimeout(retryTimer);
    retryTimer = undefined;
    if (held) release(held);
    // A subsequent visit is a new terminal demand.
    failure = undefined;
    retries = 0;
  }

  return {
    owner,
    ensure,
    idle,
    carrier: () => held?.carrier,
    async use(work) {
      operations++;
      try { return await work(await ensure({ manual: true })); }
      finally { operations--; idle(); }
    },
    call(carrier, method, params, options) {
      if (carrier !== held?.carrier || !owner.isCurrent()) return Promise.reject(httpFailure("scope_ended"));
      return carrier.call(method, params, { timeoutMs: CALL_TIMEOUT, ...options });
    },
    close() {
      ended = true;
      clearTimeout(retryTimer);
      if (held) release(held);
    },
  };
}
