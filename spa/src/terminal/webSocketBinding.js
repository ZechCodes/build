import { httpFailure } from "../core/httpEndpoint.js";

const RETRY_DELAYS = [250, 500, 1000, 2000, 4000, 8000];

export function terminalRetryDelay(retries) {
  const delay = RETRY_DELAYS[Math.min(retries, RETRY_DELAYS.length - 1)];
  return Math.round(delay * (0.8 + Math.random() * 0.2));
}
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
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      if (hasPanes()) void ensure().catch(ignore);
    }, terminalRetryDelay(retries++));
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
