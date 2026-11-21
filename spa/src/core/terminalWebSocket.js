import { endedOpeningError } from "./bridgeEpoch.js";
import { httpFailure } from "./httpEndpoint.js";

export const TERMINAL_PROTOCOL = "build.term.v1";

const CLIENT_BYTES = 128 * 1024;
const ORDINARY_BYTES = 1024 * 1024;
const SNAPSHOT_BYTES = 12 * 1024 * 1024;
const SNAPSHOT_METADATA_BYTES = 64 * 1024;
const INPUT_BYTES = 64 * 1024;
const SEND_QUEUE_BYTES = 2 * 1024 * 1024;
const SEND_STALL_MS = 10000;
const CALL_LIMIT = 8;
const QUEUE_ITEMS = 128;
const MAX_DEPTH = 16;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const idPattern = /^[A-Za-z0-9_-]{1,128}$/u;
const base64Alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const attachMethods = new Set(["term.attach", "agent.attach"]);
const scope = ["workspace_id", "run_id", "project_id", "worktree_id", "source_id", "source_path"];
const fields = new Map([
  ["term.create", ["kind", "cols", "rows", ...scope]],
  ["term.attach", ["term_id", "cols", "rows", ...scope]],
  ["term.input", ["term_id", "data"]],
  ["term.resize", ["term_id", "cols", "rows"]],
  ["term.ack", ["term_id", "cursor"]],
  ["term.close", ["term_id", ...scope]],
  ["agent.attach", ["id", "agent_id", "conversation_id", "cols", "rows", ...scope]],
  ["ping", []],
]);

const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value) => typeof value === "string" && value.length > 0;
const validId = (value) => typeof value === "string" && idPattern.test(value);
const cursor = (value) => Number.isSafeInteger(value) && value >= 0;
const axis = (value, max) => Number.isInteger(value) && value >= 1 && value <= max;
const dimension = (params, key, fallback) => Object.hasOwn(params, key) ? params[key] : fallback;

function base64Bytes(value) {
  if (typeof value !== "string" || value.length % 4 || !/^[A-Za-z0-9+/]*={0,2}$/u.test(value)) return -1;
  const padding = value.endsWith("==") ? 2 : Number(value.endsWith("="));
  if (padding && base64Alphabet.indexOf(value.at(-padding - 1)) % (padding === 2 ? 16 : 4)) return -1;
  return value.length * 3 / 4 - padding;
}

function withinDepth(value, depth = 1) {
  if (value === null || typeof value !== "object") return true;
  if (depth > MAX_DEPTH) return false;
  for (const key of Object.keys(value)) if (!withinDepth(value[key], depth + 1)) return false;
  return true;
}

function validGrid(params, defaultCols, defaultRows) {
  const cols = dimension(params, "cols", defaultCols);
  const rows = dimension(params, "rows", defaultRows);
  return axis(cols, 512) && axis(rows, 256) && cols * rows <= 65536;
}

function checkParams(method, params) {
  const allowed = fields.get(method);
  if (!allowed) throw httpFailure("unknown_method");
  if (!object(params) || !Object.keys(params).every((key) => allowed.includes(key))) throw httpFailure("invalid_params");
  if (allowed.includes("cols")) {
    const agent = method === "agent.attach";
    if (!validGrid(params, agent ? 120 : 80, agent ? 40 : 24)) throw httpFailure("invalid_params");
  }
  if (method === "term.ack" && !cursor(params.cursor)) throw httpFailure("invalid_params");
}

const validGridResult = ({ cols, rows }) => axis(cols, 512) && axis(rows, 256) && cols * rows <= 65536;
const validScreen = (frame) => text(frame.term_id) && cursor(frame.cursor);

function validAttach(result) {
  return object(result) && validScreen(result) && validGridResult(result) && base64Bytes(result.snapshot) >= 0;
}

function validOutput(frame) {
  const bytes = base64Bytes(frame.data);
  return validScreen(frame) && bytes >= 0 && bytes <= frame.cursor;
}

function validOrdinaryPush(frame) {
  switch (frame.type) {
    case "term.output": return validOutput(frame);
    case "term.closed": return text(frame.term_id) && text(frame.reason);
    case "attachment_error": return validId(frame.attach_id) && frame.reason === "snapshot_too_large";
    default: return false;
  }
}

function readFrame(data) {
  if (typeof data === "string") throw protocolError(1003);
  if (!(data instanceof ArrayBuffer)) throw protocolError();
  if (data.byteLength > SNAPSHOT_BYTES) throw protocolError(1009);
  let decoded;
  try { decoded = decoder.decode(data); } catch { throw protocolError(1007); }
  let frame;
  try { frame = JSON.parse(decoded); } catch { throw protocolError(); }
  if (!object(frame) || !withinDepth(frame)) throw protocolError();
  return frame;
}

function snapshotPayload(frame, method) {
  if (frame.type === "term.reset" && !Object.hasOwn(frame, "id") && validScreen(frame) && base64Bytes(frame.data) >= 0) {
    return frame.data;
  }
  if (attachMethods.has(method) && frame.ok === true && !Object.hasOwn(frame, "type") && validAttach(frame.result)) {
    return frame.result.snapshot;
  }
  return undefined;
}

function admitFrame(frame, byteLength, method) {
  const payload = snapshotPayload(frame, method);
  if (payload !== undefined) {
    if (byteLength - payload.length > SNAPSHOT_METADATA_BYTES) throw protocolError(1009);
    return;
  }
  if (byteLength > ORDINARY_BYTES) throw protocolError(1009);
  if (Object.hasOwn(frame, "id")) checkOrdinaryReply(frame, method);
  else if (!validOrdinaryPush(frame)) throw protocolError();
}

function checkOrdinaryReply(frame, method) {
  if (!validId(frame.id) || Object.hasOwn(frame, "type")) throw protocolError();
  if (frame.ok === true && Object.hasOwn(frame, "result") && !attachMethods.has(method)) return;
  if (frame.ok === false && text(frame.error) && text(frame.error_code)) return;
  throw protocolError();
}

function refusal(frame) {
  return Object.assign(new Error(frame.error), {
    error_code: frame.error_code, retryable: frame.retryable === true,
    details: frame.details, uncertain: false, bridgeRefusal: true,
  });
}

function protocolError(closeCode = 1002) {
  return Object.assign(new Error("The bridge returned an invalid terminal WebSocket message."), {
    error_code: "protocol", code: "protocol", closeCode, retryable: false, uncertain: false,
  });
}

function transportError(closeCode = 1006) {
  const retryable = ![1002, 1003, 1007, 1008, 1009].includes(closeCode);
  return Object.assign(new Error("The terminal WebSocket connection ended."), {
    error_code: "terminal_disconnected", code: "terminal_disconnected", closeCode, retryable, uncertain: false,
  });
}

function busy() { return Object.assign(httpFailure("busy"), { retryable: true }); }

function emit(listeners, ...args) {
  for (const listener of [...listeners]) {
    if (!listeners.has(listener)) continue;
    try { listener(...args); } catch { /* A consumer cannot interrupt delivery or teardown for siblings. */ }
  }
}

function callerResult(wire, method, { signal, timeoutMs }) {
  if (!signal && !(timeoutMs > 0)) return wire;
  return new Promise((resolve, reject) => {
    const cancel = () => reject(endedOpeningError(method, true));
    const timer = timeoutMs > 0 ? setTimeout(() => reject(terminalTimeout()), timeoutMs) : undefined;
    const cleanup = () => { clearTimeout(timer); signal?.removeEventListener("abort", cancel); };
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    wire.then(resolve, reject).finally(cleanup);
  });
}

function terminalTimeout() {
  return Object.assign(httpFailure("timeout", true), {
    message: "The terminal WebSocket did not answer before the deadline.", timedOut: true, deadline: "path",
  });
}

export async function openTerminalWebSocket({ url, issueTicket, createWebSocket, epoch, signal, isCurrent, retiredError, onEnd }) {
  let ticket;
  try { ticket = await issueTicket(signal); } catch (error) {
    if (signal.aborted || !isCurrent()) throw retiredError();
    if (error.code === "network" || error.code === "timeout") {
      error.retryable = true;
      error.uncertain = false;
    }
    throw error;
  }
  if (signal.aborted || !isCurrent()) throw retiredError();
  let socket;
  try { socket = createWebSocket(url, [TERMINAL_PROTOCOL, `ticket.${ticket.token}`]); }
  catch { throw transportError(); }
  return createCarrier({ socket, epoch, signal, isCurrent, retiredError, onEnd });
}

function createCarrier({ socket, epoch, signal, isCurrent, retiredError, onEnd }) {
  const held = new Map();
  const pushes = new Set();
  const closes = new Set();
  const ready = Promise.withResolvers();
  let terminalError;
  let requestId = 0;
  let stallTimer;
  let lastBuffered = 0;
  const current = () => !terminalError && !signal.aborted && isCurrent();

  const stop = (error) => {
    if (terminalError) return;
    terminalError = error;
    clearTimeout(openTimer);
    clearTimeout(stallTimer);
    signal.removeEventListener("abort", retire);
    socket.removeEventListener("open", opened);
    socket.removeEventListener("message", message);
    socket.removeEventListener("close", lost);
    // Browser WebSocket.close() only accepts 1000 or application codes.
    // Keep the protocol classification for the local owner.
    try { socket.close(); } catch { /* The transport already ended. */ }
    ready.reject(error);
    for (const [id, call] of held) {
      held.delete(id);
      call.reject(endedOpeningError(call.method, true));
    }
    pushes.clear();
    onEnd();
    try { emit(closes, error); } finally { closes.clear(); }
  };
  const retire = () => stop(retiredError());
  const lost = ({ code }) => stop(current() ? transportError(code) : retiredError());
  const openTimer = setTimeout(() => stop(current() ? transportError() : retiredError()), SEND_STALL_MS);
  const openSocket = () => {
    if (!current()) {
      retire();
      throw httpFailure("scope_ended");
    }
    if (socket.readyState !== 1) throw transportError();
  };

  const monitorSend = (buffered) => {
    clearTimeout(stallTimer);
    lastBuffered = buffered;
    stallTimer = buffered > 0 ? setTimeout(checkStall, SEND_STALL_MS) : undefined;
  };
  function checkStall() {
    if (!current()) { retire(); return; }
    const buffered = socket.bufferedAmount;
    if (buffered > 0 && buffered >= lastBuffered) { stop(transportError()); return; }
    monitorSend(buffered);
  }
  const send = (frame) => {
    openSocket();
    if (!withinDepth(frame)) throw httpFailure("invalid_params");
    const bytes = encoder.encode(JSON.stringify(frame));
    if (bytes.length > CLIENT_BYTES) throw httpFailure("request_too_large");
    const before = socket.bufferedAmount;
    if (before + bytes.length > SEND_QUEUE_BYTES) {
      const error = transportError();
      stop(error);
      throw error;
    }
    try { socket.send(bytes); } catch {
      const error = transportError();
      stop(error);
      throw error;
    }
    // New sends extend the byte watermark without extending the deadline.
    const after = socket.bufferedAmount;
    if (stallTimer === undefined || before < lastBuffered || after === 0) monitorSend(after);
    else lastBuffered = after;
  };

  const sendCall = (method, params, options = {}) => {
    const { signal: callerSignal, onAttachId, assertCurrent } = options;
    openSocket();
    assertCurrent?.();
    if (callerSignal?.aborted) throw httpFailure("canceled");
    if (held.size >= CALL_LIMIT) throw busy();
    const id = String(++requestId);
    const reply = Promise.withResolvers();
    send({ id, method, params });
    held.set(id, { ...reply, method });
    const result = callerResult(reply.promise, method, options);
    if (onAttachId && attachMethods.has(method)) {
      try { onAttachId(id); } catch (error) { void result.catch(() => {}); throw error; }
    }
    return result;
  };
  const input = orderedInput(sendCall, openSocket);
  const listen = (listeners, callback) => {
    listeners.add(callback);
    return () => listeners.delete(callback);
  };
  const carrier = {
    async call(method, params = {}, options = {}) {
      checkParams(method, params);
      if (method === "term.input") return input(params, options);
      return sendCall(method, params, options);
    },
    detach(attachId) {
      if (!validId(attachId) || !current() || socket.readyState !== 1) return;
      try { send({ type: "detach", attach_id: attachId }); } catch { /* Send failure retires this socket's interests. */ }
    },
    onPush: (callback) => listen(pushes, callback),
    onClose(callback) {
      if (terminalError) { callback(terminalError); return () => {}; }
      return listen(closes, callback);
    },
    close: retire,
  };

  function opened() {
    if (!current()) { retire(); return; }
    if (socket.protocol !== TERMINAL_PROTOCOL || socket.extensions) { stop(protocolError()); return; }
    clearTimeout(openTimer);
    ready.resolve(carrier);
  }
  function message({ data }) {
    if (!current()) { retire(); return; }
    let frame;
    let call;
    try {
      frame = readFrame(data);
      if (frame.bridge_epoch !== epoch) throw protocolError();
      call = held.get(frame.id);
      admitFrame(frame, data.byteLength, call?.method);
    } catch (error) { stop(error); return; }
    if (!Object.hasOwn(frame, "id")) { emit(pushes, frame); return; }
    if (call) {
      held.delete(frame.id);
      if (frame.ok) call.resolve(frame.result);
      else call.reject(refusal(frame));
    }
  }

  socket.binaryType = "arraybuffer";
  socket.addEventListener("open", opened);
  socket.addEventListener("message", message);
  socket.addEventListener("close", lost);
  signal.addEventListener("abort", retire, { once: true });
  if (!current()) retire();
  return ready.promise;
}

function orderedInput(call, assertOpen) {
  const tails = new Map();
  let queuedBytes = 0;
  let queuedItems = 0;
  return ({ term_id: termId, data }, options) => {
    assertOpen();
    const params = { term_id: termId, data };
    const bytes = inputSize(data);
    const tail = tails.get(termId);
    if (!tail && bytes <= INPUT_BYTES) return call("term.input", params, options);
    if (queuedBytes + bytes > SEND_QUEUE_BYTES || queuedItems >= QUEUE_ITEMS) throw busy();
    queuedBytes += bytes;
    queuedItems += 1;
    const run = () => sendInputChunks(call, params, options);
    const sending = tail ? tail.then(run, run) : run();
    const done = sending.finally(() => {
      queuedBytes -= bytes;
      queuedItems -= 1;
      if (tails.get(termId) === done) tails.delete(termId);
    });
    tails.set(termId, done);
    return done;
  };
}

function inputSize(data) {
  if (typeof data === "string" && data.length > SEND_QUEUE_BYTES) throw httpFailure("request_too_large");
  const bytes = base64Bytes(data);
  if (bytes < 0) throw httpFailure("invalid_params");
  return bytes;
}

async function sendInputChunks(call, params, options) {
  const bytes = atob(params.data);
  let offset = 0;
  let result;
  try {
    do {
      const data = btoa(bytes.slice(offset, offset + INPUT_BYTES));
      result = await call("term.input", { ...params, data }, options);
      offset += INPUT_BYTES;
    } while (offset < bytes.length);
    return result;
  } catch (error) {
    if (offset > 0) error.uncertain = true;
    throw error;
  }
}
