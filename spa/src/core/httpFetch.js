import { httpFailure } from "./httpEndpoint.js";

export async function boundedJson(response, limit = 16 * 1024 * 1024) {
  if (!response.body) throw httpFailure("protocol");
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let size = 0;
  let text = "";
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw httpFailure("protocol");
      text += decoder.decode(value, { stream: true });
    }
    return JSON.parse(text + decoder.decode());
  } catch {
    throw httpFailure("protocol");
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

// Own the signal across headers AND body. Superseded/account-reset callers
// cannot adopt a response even if a test double or platform ignores abort.
export async function httpRequest(run, { signal, timeoutMs = 12000, isCurrent = () => true } = {}) {
  const controller = new AbortController();
  const abort = () => controller.abort(signal.reason);
  signal?.addEventListener("abort", abort, { once: true });
  if (signal?.aborted) abort();
  let timedOut = false;
  const timer = timeoutMs > 0 ? setTimeout(() => {
    timedOut = true; controller.abort();
  }, timeoutMs) : null;
  const check = () => {
    if (!isCurrent()) throw httpFailure("scope_ended");
    if (timedOut) throw Object.assign(httpFailure("timeout"), { timedOut: true, deadline: "path" });
    if (controller.signal.aborted) throw signal?.reason || new DOMException("Request cancelled", "AbortError");
  };
  let stopAbort;
  const cancelled = new Promise((_, reject) => {
    stopAbort = () => { try { check(); } catch (error) { reject(error); } };
    controller.signal.addEventListener("abort", stopAbort, { once: true });
  });
  try {
    check();
    const result = await Promise.race([run(controller.signal, check), cancelled]);
    check();
    return result;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
    controller.signal.removeEventListener("abort", stopAbort);
  }
}

export const jsonHeaders = Object.freeze({ "Content-Type": "application/json", Accept: "application/json" });
