import { httpFailure } from "./httpEndpoint.js";
import { httpRequest } from "./httpFetch.js";

export function desktopHttpValue(envelope) {
  if (envelope?.ok === true) return envelope.value;
  const error = envelope?.error;
  const failure = httpFailure(typeof error?.code === "string" ? error.code : "protocol", error?.uncertain === true);
  throw failure;
}

export function createDesktopHttpEndpoint({ api, identity, isCurrent = () => true }) {
  let closed = false;
  let subscription;
  let subscribeStarted = false;
  let onLost = () => {};
  const current = () => !closed && isCurrent();
  const invoke = async (name, ...args) => {
    if (!current()) throw httpFailure("scope_ended");
    const result = desktopHttpValue(await api[name](...args));
    if (!current()) throw httpFailure("scope_ended");
    return result;
  };
  const quiet = (name, ...args) => { try { void Promise.resolve(api[name](...args)).catch(() => {}); } catch { /* Native frame may already be gone. */ } };
  const close = () => {
    if (closed) return;
    closed = true;
    subscription?.close();
    quiet("close", identity.handle);
  };
  const lose = () => { if (!closed) { close(); onLost(httpFailure("stream_reset")); } };

  const subscribe = async (_opening, consume) => {
    // One immutable Opening per endpoint: recovery creates a new native handle.
    // Never overlap or retry a subscription on this handle, even before its
    // stream UUID arrives. Native also rejects concurrent handle subscriptions.
    if (subscribeStarted) throw httpFailure("protocol");
    subscribeStarted = true;
    const requestId = crypto.randomUUID();
    let stream;
    let sequence = 0;
    let buffered = [];
    let bytes = 0;
    let count = 0;
    let resolveReady;
    const ready = new Promise((resolve) => { resolveReady = resolve; });
    let applying = Promise.resolve();
    const deliver = async (message) => {
      if (!current() || message.stream !== stream) return;
      if (!Number.isSafeInteger(message.sequence) || message.sequence !== sequence + 1) throw httpFailure("protocol");
      sequence = message.sequence;
      await consume({ event: message.event, data: message.data });
      if (!current()) return;
      desktopHttpValue(await api.ack(stream, message.sequence));
      if (message.event === "hello") resolveReady();
    };
    const enqueue = (message) => {
      // onEvent broadcasts every native stream. Both owner and attempt are
      // known before subscribe resolves; stale/missing correlation must never
      // enter this queue or consume its item/byte allowance (including controls).
      if (!current() || message?.handle !== identity.handle || message.requestId !== requestId) return;
      if (stream && message.stream !== stream) return;
      bytes += messageBytes(message); count += 1;
      if (count > 128 || bytes > 1024 * 1024) { lose(); return; }
      buffered.push(message);
      if (!stream) return;
      applying = applying.then(async () => {
        const held = buffered.shift();
        if (!held) return;
        try { await deliver(held); }
        finally { bytes -= messageBytes(held); count -= 1; }
      }).catch(lose);
    };
    const stopEvents = api.onEvent(enqueue);
    try {
      const result = await invoke("subscribe", identity.handle, { kind: "application" }, { requestId });
      if (typeof result?.stream !== "string") throw httpFailure("protocol");
      stream = result.stream;
      const early = buffered;
      buffered = []; bytes = 0; count = 0;
      subscription = { binding: stream, close() { stopEvents(); quiet("unsubscribe", stream); buffered = []; } };
      for (const message of early) enqueue(message);
      await httpRequest(() => ready, { timeoutMs: 5000, isCurrent: current });
      return subscription;
    } catch (error) { stopEvents(); if (stream) quiet("unsubscribe", stream); throw error; }
  };

  return {
    identity,
    open: (apiRange) => invoke("open", identity.handle, apiRange),
    onLost: (listener) => { onLost = listener; },
    async request(_opening, route, options) {
      const requestId = options.requestId || crypto.randomUUID();
      const abort = () => quiet("cancel", requestId);
      if (options.signal?.aborted) throw httpFailure("scope_ended");
      options.signal?.addEventListener("abort", abort, { once: true });
      try {
        return await invoke("request", identity.handle, route.operationId, route.params, {
          requestId, priority: options.priority || "foreground", ...(options.write ? { write: options.write } : {}),
        });
      } finally { options.signal?.removeEventListener("abort", abort); }
    },
    subscribe, close,
  };
}

const messageBytes = (message) => new TextEncoder().encode(JSON.stringify(message)).length;
