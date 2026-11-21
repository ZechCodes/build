import { endedOpeningError } from "./bridgeEpoch.js";
import { httpFailure } from "./httpEndpoint.js";
import { httpRequest } from "./httpFetch.js";

const terminalApi = ["terminalOpen", "terminalCall", "terminalConsume", "terminalDetach", "terminalClose", "onTerminalEvent"];
const supportsNativeTerminal = (api) => terminalApi.every((name) => typeof api[name] === "function");
const attaches = new Set(["term.attach", "agent.attach"]);
const effects = new Set(["term.create", "term.input", "term.resize", "term.close"]);
const transient = new Set([
  "busy", "terminal_busy", "terminal_network_error", "terminal_binding_changed",
  "terminal_credentials_rotated", "terminal_delivery_timeout", "terminal_delivery_overflow",
  "terminal_queue_full", "terminal_ticket_expired", "network_error", "timeout",
]);

function terminalFailure({ code, status, retryable, closeCode, reason }) {
  const error = httpFailure(code);
  error.message = terminalFailureMessage(code, status) ?? error.message;
  if (status !== undefined) error.httpStatus = status;
  if (closeCode !== undefined) error.closeCode = closeCode;
  if (reason !== undefined) error.reason = reason;
  error.retryable = retryable ?? (transient.has(code) || retryableClose(code, closeCode));
  return error;
}

function terminalFailureMessage(code, status) {
  if (status === 429) return "Terminal connection limit reached";
  if (status === 401) return httpFailure("authentication").message;
  if (code === "forbidden_scope" || code === "terminal_forbidden")
    return "This device grant does not allow terminal access.";
}

function retryableClose(code, closeCode) {
  return code === "terminal_closed" && ![1002, 1003, 1007, 1008, 1009, 4008].includes(closeCode);
}

function terminalValue(envelope) {
  if (envelope?.ok === true) return envelope.value;
  const error = terminalFailure(envelope?.error || { code: "protocol" });
  error.uncertain = envelope?.error?.uncertain === true;
  throw error;
}

function openingIdentity(opening) {
  return {
    session: opening?.session_id,
    generation: opening?.opening_generation,
    epoch: opening?.greeting?.bridge_epoch,
  };
}

function sameTerminalOpening(held, offered) {
  return held?.session && held.generation && held.epoch &&
    held.session === offered.session && held.generation === offered.generation && held.epoch === offered.epoch;
}

export function createDesktopTerminalAccess({ api, identity, current }) {
  if (!supportsNativeTerminal(api)) return null;
  let opening;
  let attempt;
  const close = () => attempt?.close();
  return {
    bind(value) {
      close();
      opening = openingIdentity(value);
    },
    async open(heldOpening, { signal, retry } = {}) {
      if (!current() || signal?.aborted || !sameTerminalOpening(opening, openingIdentity(heldOpening))) throw httpFailure("scope_ended");
      const explicitRetry = retry === true;
      if (attempt) {
        if (attempt.signal !== signal || attempt.retry !== explicitRetry) throw terminalFailure({ code: "busy" });
        return attempt.ready;
      }
      const next = { signal, retry: explicitRetry };
      attempt = next;
      const release = () => { if (attempt === next) attempt = undefined; };
      const ticketBody = { opening_generation: opening.generation, ...(explicitRetry && { retry: true }) };
      Object.assign(next, createDesktopTerminal({
        api, handle: identity.handle, epoch: opening.epoch, signal, ticketBody,
        isCurrent: () => current() && attempt === next, release,
      }));
      return next.ready;
    },
    close,
  };
}

function safelyInvoke(work) {
  try { return Promise.resolve(work()); }
  catch (error) { return Promise.reject(error); }
}

function notify(listeners, value) {
  for (const listener of [...listeners]) {
    if (listeners.has(listener)) void safelyInvoke(() => listener(value)).catch(() => {});
  }
}

const RECEIVE_STALL_MS = 24_000;
// A snapshot is delivered whole. Leave room for D2's physical write deadline
// even if the individual caller has already timed out.
const SNAPSHOT_RECEIVE_STALL_MS = 202_000;
const IDLE_PROBE_MS = 5000;
const DELIVERY_ITEMS = 128;

function createDesktopTerminal({ api, handle, epoch, signal, ticketBody, isCurrent, release }) {
  const requestId = crypto.randomUUID();
  const ready = Promise.withResolvers();
  const held = new Map();
  const pushes = new Set();
  const closes = new Set();
  let early = [];
  let terminal;
  let error;
  let stopEvents;
  let liveness;
  let sequence = 0;
  let lastId = 0;
  let consuming = false;

  const current = () => !error && !signal?.aborted && isCurrent();
  const closeNative = (id) => { void safelyInvoke(() => api.terminalClose(id)).catch(() => {}); };
  const retire = () => stop(httpFailure("scope_ended"));
  const listen = (listeners, listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };

  function stop(failure) {
    if (error) return;
    error = failure;
    early = [];
    clearTimeout(liveness);
    signal?.removeEventListener("abort", retire);
    stopEvents?.();
    closeNative(terminal ?? requestId);
    for (const pending of held.values()) pending.reject(endedOpeningError(pending.method, effects.has(pending.method)));
    held.clear();
    pushes.clear();
    release();
    ready.reject(failure);
    notify(closes, failure);
    closes.clear();
  }

  function watchLiveness() {
    clearTimeout(liveness);
    if (!terminal || !current()) return;
    if (!held.size) {
      liveness = setTimeout(() => { void carrier.call("ping").catch(() => {}); }, IDLE_PROBE_MS);
      return;
    }
    const snapshot = [...held.values()].some(({ method }) => attaches.has(method));
    liveness = setTimeout(
      () => stop(current() ? terminalFailure({ code: "terminal_network_error" }) : httpFailure("scope_ended")),
      snapshot ? SNAPSHOT_RECEIVE_STALL_MS : RECEIVE_STALL_MS,
    );
  }

  function consume(message) {
    void safelyInvoke(() => api.terminalConsume(terminal, message.sequence)).then(terminalValue).catch((failure) => {
      // The close event may already be in flight when native withdraws the handle.
      if (current() && failure.code !== "invalid_terminal_handle") stop(failure);
    });
  }

  function deliver(message) {
    if (!current() || message.terminal !== terminal) return;
    if (message.type === "close") { stop(terminalFailure(message)); return; }
    const frame = message.data;
    if (message.type !== "message" || message.sequence !== sequence + 1 || frame?.bridge_epoch !== epoch) {
      stop(httpFailure("protocol"));
      return;
    }
    sequence = message.sequence;
    if (Object.hasOwn(frame, "id")) settleTerminalReply(held, frame);
    else notify(pushes, frame);
    // A listener has taken the frame or staged it for paint. IPC credit is
    // independent of the emulator's later term.ack cursor.
    if (current()) consume(message);
    watchLiveness();
  }

  function receive(message) {
    if (!current() || message?.requestId !== requestId) return;
    if (terminal && (consuming || message.type === "close")) { deliver(message); return; }
    if (terminal && terminal !== message.terminal) return;
    if (early.length >= DELIVERY_ITEMS + 1) { stop(httpFailure("protocol")); return; }
    early.push(message);
  }

  function beginDelivery() {
    if (consuming) return;
    consuming = true;
    const buffered = early;
    early = [];
    for (const message of buffered) deliver(message);
  }

  function assertOpen() {
    if (current()) return;
    retire();
    throw error;
  }

  function sendCall(method, params, options) {
    beginDelivery();
    assertOpen();
    options.assertCurrent?.();
    if (options.signal?.aborted) throw httpFailure("canceled");
    if (held.size >= 8) throw terminalFailure({ code: "busy" });
    const id = String(++lastId);
    const pending = { ...Promise.withResolvers(), method };
    held.set(id, pending);
    if (held.size === 1) watchLiveness();
    void safelyInvoke(() => api.terminalCall(terminal, method, params, id)).then(terminalValue).catch((failure) => {
      if (!held.delete(id)) return;
      pending.reject(failure);
      if (!held.size) watchLiveness();
    });
    const result = waitForTerminalReply(pending.promise, method, options);
    if (options.onAttachId && attaches.has(method)) {
      try { options.onAttachId(id); } catch (failure) { void result.catch(() => {}); throw failure; }
    }
    return result;
  }

  const input = orderedDesktopInput(sendCall);
  const carrier = {
    async call(method, params = {}, options = {}) {
      if (method === "term.input") return input(params, options);
      return sendCall(method, params, options);
    },
    detach(id) {
      if (current()) void safelyInvoke(() => api.terminalDetach(terminal, id)).catch(() => {});
    },
    onPush(listener) {
      if (!current()) return () => {};
      const unsubscribe = listen(pushes, listener);
      beginDelivery();
      return unsubscribe;
    },
    onClose(listener) {
      if (error) { listener(error); return () => {}; }
      return listen(closes, listener);
    },
    close: retire,
  };

  async function open() {
    try {
      stopEvents = api.onTerminalEvent(receive);
      signal?.addEventListener("abort", retire, { once: true });
      assertOpen();
      const value = terminalValue(await api.terminalOpen(handle, requestId, ticketBody));
      if (typeof value?.terminal !== "string") throw httpFailure("protocol");
      if (error) { closeNative(value.terminal); return; }
      terminal = value.terminal;
      assertOpen();
      const earlyClose = early.find((message) => message.terminal === terminal && message.type === "close");
      if (earlyClose) { deliver(earlyClose); return; }
      watchLiveness();
      ready.resolve(carrier);
    } catch (failure) {
      // Native can withdraw the handle before its open invocation settles.
      // If the frame received the close control first, preserve its reason.
      const earlyClose = early.find(({ type }) => type === "close");
      stop(earlyClose ? terminalFailure(earlyClose) : failure);
    }
  }

  void open();
  return { ready: ready.promise, close: retire };
}

function settleTerminalReply(held, frame) {
  const pending = held.get(frame.id);
  if (!pending) return;
  held.delete(frame.id);
  if (frame.ok) pending.resolve(frame.result);
  else pending.reject(Object.assign(new Error(frame.error), {
    code: frame.error_code, error_code: frame.error_code, retryable: frame.retryable,
    details: frame.details, uncertain: false, bridgeRefusal: true,
  }));
}

function waitForTerminalReply(reply, method, { signal, timeoutMs = 0 }) {
  if (!signal && !(timeoutMs > 0)) return reply;
  void reply.catch(() => {});
  return httpRequest(() => reply, { signal, timeoutMs }).catch((error) => {
    if (signal?.aborted) throw endedOpeningError(method, effects.has(method));
    if (error.timedOut) Object.assign(error, { uncertain: effects.has(method), retryable: true });
    throw error;
  });
}

const INPUT_BYTES = 64 * 1024;
const INPUT_QUEUE = 2 * 1024 * 1024;

function orderedDesktopInput(call) {
  const tails = new Map();
  let queuedBytes = 0;
  let queuedItems = 0;
  return (params, options) => {
    const { data, term_id: termId } = params;
    if (typeof data !== "string") throw httpFailure("invalid_params");
    if (data.length > INPUT_QUEUE) throw httpFailure("request_too_large");
    const tail = tails.get(termId);
    const bytes = data.length / 4 * 3 - (data.endsWith("==") ? 2 : Number(data.endsWith("=")));
    if (!tail && bytes <= INPUT_BYTES) return call("term.input", params, options);
    if (queuedBytes + data.length > INPUT_QUEUE || queuedItems >= DELIVERY_ITEMS) throw terminalFailure({ code: "busy" });
    queuedBytes += data.length;
    queuedItems += 1;
    const send = () => sendDesktopInput(call, params, options, bytes);
    // A later keystroke can still be sent after the previous paste fails.
    // sendCall rechecks authority before sending it on the same carrier.
    const result = (tail ? tail.then(send, send) : send()).finally(() => {
      queuedBytes -= data.length;
      queuedItems -= 1;
      if (tails.get(termId) === result) tails.delete(termId);
    });
    tails.set(termId, result);
    return result;
  };
}

async function sendDesktopInput(call, params, options, bytes) {
  if (bytes <= INPUT_BYTES) return call("term.input", params, options);
  const binary = decodeDesktopInput(params.data);
  let offset = 0;
  let result;
  try {
    for (; offset < binary.length; offset += INPUT_BYTES) {
      const data = btoa(binary.slice(offset, offset + INPUT_BYTES));
      result = await call("term.input", { ...params, data }, options);
    }
    return result;
  } catch (error) {
    if (offset > 0) error.uncertain = true;
    throw error;
  }
}

function decodeDesktopInput(data) {
  let binary;
  try { binary = atob(data); } catch { throw httpFailure("invalid_params"); }
  if (btoa(binary) !== data) throw httpFailure("invalid_params");
  return binary;
}
