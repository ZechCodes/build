import { isUtf8 } from "node:buffer";
import { randomUUID } from "node:crypto";
import WebSocket from "ws";
import { exact, fail, object } from "./bridge-http-contract.mjs";
import { MAX_JSON_BYTES, transportError } from "./bridge-http-network.mjs";

const PROTOCOL = "build.term.v1";
const CAPABILITY = "term.websocket";
const MAX_CLIENT_BYTES = 128 * 1024;
const MAX_INPUT_BYTES = 64 * 1024;
const MAX_SNAPSHOT_BYTES = 12 * 1024 * 1024;
const MAX_SNAPSHOT_METADATA = 64 * 1024;
const MAX_DEPTH = 16;
const MAX_CALLS = 8;
const MAX_QUEUED_WRITES = 128;
const STALL_MS = 10_000;
const TICKET_MS = 15_000;
const REQUEST_ID = /^[A-Za-z0-9_-]{1,128}$/u;
const HTTP_TOKEN = /^[!#$%&'*+.^_\x60|~0-9A-Za-z-]+$/u;
const REFUSALS = new Map([[401, "unauthorized"], [403, "terminal_forbidden"], [429, "terminal_capacity"]]);
const CODEC_CLOSE = new Map([
  ["WS_ERR_UNSUPPORTED_MESSAGE_LENGTH", 1009],
  ["WS_ERR_UNSUPPORTED_DATA_PAYLOAD_LENGTH", 1009],
  ["WS_ERR_INVALID_UTF8", 1007],
  ["WS_ERR_TOO_MANY_BUFFERED_PARTS", 1008],
]);

const SELECTORS = ["workspace_id", "run_id", "project_id", "worktree_id", "source_id", "source_path"];
const PARAMS = new Map([
  ["term.create", ["kind", "cols", "rows", ...SELECTORS]],
  ["term.attach", ["term_id", "cols", "rows", ...SELECTORS]],
  ["term.input", ["term_id", "data"]],
  ["term.resize", ["term_id", "cols", "rows"]],
  ["term.ack", ["term_id", "cursor"]],
  ["term.close", ["term_id", ...SELECTORS]],
  ["agent.attach", ["id", "agent_id", "conversation_id", "cols", "rows", ...SELECTORS]],
  ["ping", []],
]);
const ATTACH = new Set(["term.attach", "agent.attach"]);
const SIZED_METHODS = new Set([...ATTACH, "term.create", "term.resize"]);
const EFFECTS = new Set(["term.create", "term.input", "term.resize", "term.close"]);

export function terminalMode(opening) {
  const descriptor = opening.transport.terminal_websocket;
  if (descriptor === undefined) return "legacy";
  if (!validDescriptor(descriptor)) return "invalid";
  const { version, protocol } = descriptor;
  if (version > 1) return protocol === PROTOCOL ? "invalid" : "legacy";
  const capabilities = opening.greeting.capabilities;
  return protocol === PROTOCOL && Array.isArray(capabilities) && capabilities.includes(CAPABILITY)
    ? "wss"
    : "invalid";
}

function validDescriptor(value) {
  return value && Number.isSafeInteger(value.version) && value.version >= 1 &&
    typeof value.protocol === "string" && HTTP_TOKEN.test(value.protocol);
}

function wireId(value) {
  return typeof value === "string" && REQUEST_ID.test(value);
}

function ticketDeadline(value, generation) {
  exact(value, ["ticket", "expires_at", "opening_generation"], "invalid_response");
  if (value.opening_generation !== generation || !canonicalTicket(value.ticket))
    fail("invalid_response");
  if (typeof value.expires_at !== "string" ||
      !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/u.test(value.expires_at))
    fail("invalid_response");
  const deadline = Date.parse(value.expires_at);
  if (!Number.isFinite(deadline) || deadline <= Date.now())
    fail("invalid_response");
  return Math.min(deadline, Date.now() + TICKET_MS);
}

function canonicalTicket(value) {
  return typeof value === "string" && /^[A-Za-z0-9_-]{43}$/u.test(value) &&
    Buffer.from(value, "base64url").toString("base64url") === value;
}

function ticketResponse(response) {
  return response.status === 201 &&
    response.headers.get("content-type")?.split(";")[0].trim().toLowerCase() === "application/json" &&
    response.headers.get("cache-control")?.toLowerCase().split(",").some((directive) => directive.trim() === "no-store");
}

function refusal(status, fallback) {
  return Object.assign(transportError(REFUSALS.get(status) ?? fallback), { status });
}

async function issueTicket(device, connection, origin, body) {
  try {
    return await device(connection.handle, "/v1/terminal-tickets", {
      method: "POST",
      headers: { Origin: origin },
      body,
      maxBody: 1024,
      maxBytes: 1024,
      signal: connection.abort.signal,
      validateResponse: ticketResponse,
      preserveHandleOnUnauthorized: true,
      preserveErrorCode: true,
    });
  } catch (error) {
    if (error.code === "http_error" && error.retryable === undefined)
      throw refusal(error.status, "terminal_ticket_refused");
    throw error;
  }
}

function validUpgrade(response) {
  const { headersDistinct: fields } = response;
  return fields["sec-websocket-protocol"]?.length === 1 &&
    fields["sec-websocket-protocol"][0] === PROTOCOL &&
    fields.upgrade?.length === 1 &&
    fields["sec-websocket-accept"]?.length === 1 &&
    !fields["sec-websocket-extensions"] &&
    response.headers.connection?.split(",").some((token) => token.trim().toLowerCase() === "upgrade");
}

function websocket(origin, trustedOrigin, ticket, deadline) {
  const url = new URL("/v1/terminal", origin);
  url.protocol = "wss:";
  return new WebSocket(url, [PROTOCOL, "ticket." + ticket], {
    origin: trustedOrigin,
    followRedirects: false,
    perMessageDeflate: false,
    rejectUnauthorized: true,
    agent: false,
    handshakeTimeout: Math.max(1, deadline - Date.now()),
    closeTimeout: 1000,
    maxPayload: MAX_SNAPSHOT_BYTES,
    maxPendingAutoPongs: 128,
  });
}

function grid(cols, rows) {
  return Number.isInteger(cols) && cols > 0 && cols <= 512 &&
    Number.isInteger(rows) && rows > 0 && rows <= 256 && cols * rows <= 65_536;
}

function dimensions(method, params) {
  const agent = method === "agent.attach";
  return grid(
    Object.hasOwn(params, "cols") ? params.cols : agent ? 120 : 80,
    Object.hasOwn(params, "rows") ? params.rows : agent ? 40 : 24,
  );
}

function cursor(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function base64Bytes(data) {
  if (typeof data !== "string" || data.length % 4 !== 0) return -1;
  const padding = data.endsWith("==") ? 2 : Number(data.endsWith("="));
  const content = data.slice(0, data.length - padding);
  if (/[^A-Za-z0-9+/]/u.test(content)) return -1;
  if (padding === 2 && !/[AQgw]$/u.test(content)) return -1;
  if (padding === 1 && !/[AEIMQUYcgkosw048]$/u.test(content)) return -1;
  return data.length / 4 * 3 - padding;
}

function validateInput(data) {
  if (typeof data !== "string" || data.length > 4 * Math.ceil(MAX_INPUT_BYTES / 3))
    fail("invalid_params");
  const length = base64Bytes(data);
  if (length < 0 || length > MAX_INPUT_BYTES) fail("invalid_params");
}

function validateCall(id, method, params) {
  if (!wireId(id)) fail("invalid_params");
  const keys = PARAMS.get(method);
  if (!keys) fail("unsupported_operation");
  exact(params, keys);
  if (SIZED_METHODS.has(method) && !dimensions(method, params)) fail("invalid_params");
  if (method === "term.input") validateInput(params.data);
  if (method === "term.ack" && !cursor(params.cursor)) fail("invalid_params");
  if (!withinDepth(params, 2)) fail("invalid_params");
}

function withinDepth(value, depth = 1) {
  if (!value || typeof value !== "object") return true;
  if (depth > MAX_DEPTH) return false;
  const children = Array.isArray(value) ? value : Object.values(value);
  return children.every((child) => withinDepth(child, depth + 1));
}

function encode(value) {
  const raw = Buffer.from(JSON.stringify(value));
  if (raw.length > MAX_CLIENT_BYTES) fail("request_too_large");
  return raw;
}

function protocolError(code = "terminal_protocol_error", closeCode = 1002) {
  return Object.assign(transportError(code), { closeCode });
}

function oversized() {
  return protocolError("terminal_message_too_large", 1009);
}

function decode(data, binary) {
  if (!binary) throw protocolError("terminal_protocol_error", 1003);
  if (!isUtf8(data)) throw protocolError("terminal_protocol_error", 1007);
  try {
    return object(JSON.parse(data.toString("utf8")));
  } catch {
    throw protocolError();
  }
}

function termId(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 4096;
}

function reset(message) {
  return termId(message.term_id) && cursor(message.cursor) && base64Bytes(message.data) >= 0;
}

function output(message) {
  const length = base64Bytes(message.data);
  return termId(message.term_id) && cursor(message.cursor) && length >= 0 && length <= message.cursor;
}

function pushValid(message) {
  switch (message.type) {
    case "term.output": return output(message);
    case "term.reset": return reset(message);
    case "term.closed": return termId(message.term_id) && typeof message.reason === "string";
    case "attachment_error": return wireId(message.attach_id) && typeof message.reason === "string" && REQUEST_ID.test(message.reason);
    default: return false;
  }
}

function attachResult(value) {
  return value && termId(value.term_id) && cursor(value.cursor) &&
    grid(value.cols, value.rows) && typeof value.live === "boolean" &&
    base64Bytes(value.snapshot) >= 0;
}

function replyValid(message, request) {
  if (!wireId(message.id) || Object.hasOwn(message, "type")) return false;
  if (message.ok === true)
    return Object.hasOwn(message, "result") && (!ATTACH.has(request?.method) || attachResult(message.result));
  return message.ok === false && typeof message.error === "string" &&
    typeof message.error_code === "string" && typeof message.retryable === "boolean";
}

function snapshotLength(message, request, reply) {
  if (!reply && message.type === "term.reset") return message.data.length;
  if (reply && message.ok === true && ATTACH.has(request?.method))
    return message.result.snapshot.length;
}

function checkMessageSize(length, snapshot) {
  const limit = snapshot === undefined ? MAX_JSON_BYTES : snapshot + MAX_SNAPSHOT_METADATA;
  if (length > limit) throw oversized();
}

function parseMessage(connection, data, binary) {
  // ws maxPayload and this check bound decode cost to a 12 MiB aggregate message.
  // As in the browser endpoint, the decoded envelope determines the receive budget.
  const length = data.length;
  if (length > MAX_SNAPSHOT_BYTES) throw oversized();
  const message = decode(data, binary);
  if (message.bridge_epoch !== connection.bridgeEpoch)
    throw protocolError("terminal_epoch_mismatch");
  const request = connection.calls.get(message.id);
  const reply = Object.hasOwn(message, "id");
  const valid = reply ? replyValid(message, request) : pushValid(message);
  const snapshot = valid ? snapshotLength(message, request, reply) : undefined;
  checkMessageSize(length, snapshot);
  if (!valid || !withinDepth(message)) throw protocolError();
  return { message, request, reply, delivery: { bytes: length, snapshot: snapshot !== undefined } };
}

function callFailure(error, pending) {
  const sanitized = transportError(error.code);
  if (EFFECTS.has(pending.method)) sanitized.uncertain = true;
  return sanitized;
}

export function createBridgeTerminals({ handleFor, check, device, hostedOrigin }) {
  const terminals = new Map();

  function unlink(connection) {
    if (connection.handle.terminal === connection) delete connection.handle.terminal;
  }

  function observable(connection) {
    try {
      check(connection.handle);
      return true;
    } catch {
      return false;
    }
  }

  function notifyClose(connection, error) {
    if (!connection.opened || !observable(connection)) return;
    const { code, closeCode, reason } = error;
    deliver(connection, { type: "close", code, closeCode, ...(reason && { reason }) });
  }

  function settle(connection, error) {
    connection.done = true;
    terminals.delete(connection.id);
    connection.abort.abort();
    connection.signal?.removeEventListener("abort", connection.cancel);
    clearTimeout(connection.handshakeTimer);
    for (const timer of connection.writes) clearTimeout(timer);
    connection.writes.clear();
    connection.connected?.reject(error);
    for (const pending of connection.calls.values()) pending.reject?.(callFailure(error, pending));
    connection.calls.clear();
  }

  function end(connection, error, { notify = true, signalPeer = false } = {}) {
    if (connection.done) return;
    const socket = connection.socket;
    settle(connection, error);
    if (signalPeer && socket?.readyState === WebSocket.OPEN && socket.bufferedAmount === 0) {
      socket.close(error.closeCode);
    } else {
      unlink(connection);
      socket?.terminate();
    }
    if (notify) notifyClose(connection, error);
    connection.onEnd?.(error);
  }

  function clear(handle, code = "stale_owner", notify = false) {
    const connection = handle.terminal;
    if (!connection) return;
    if (connection.done) {
      unlink(connection);
      connection.socket.terminate();
      return;
    }
    end(connection, transportError(code), { notify });
  }

  function ensure(connection) {
    check(connection.handle);
    if (connection.done || connection.handle.terminal !== connection) fail("terminal_closed");
  }

  function currentForReceive(connection) {
    if (connection.done) return false;
    try {
      ensure(connection);
      return true;
    } catch (error) {
      end(connection, error, { notify: false });
      return false;
    }
  }

  function deliver(connection, event, delivery) {
    try {
      connection.onEvent({ terminal: connection.id, ...event }, delivery);
    } catch {
      end(connection, transportError("terminal_delivery_failed"), { notify: false });
    }
  }

  function onMessage(connection, data, binary) {
    if (!currentForReceive(connection)) return;
    let parsed;
    try {
      parsed = parseMessage(connection, data, binary);
    } catch (error) {
      const safe = error.closeCode ? error : protocolError();
      end(connection, safe, { signalPeer: observable(connection) });
      return;
    }
    if (!currentForReceive(connection)) return;
    const { message, request, reply, delivery } = parsed;
    if (reply) {
      if (!request) return;
      connection.calls.delete(message.id);
      if (!connection.forwardReplies) {
        request.resolve(message);
        return;
      }
    }
    deliver(connection, { type: "message", data: message }, delivery);
  }

  function onCodecError(connection, code) {
    const closeCode = CODEC_CLOSE.get(code) ?? 1002;
    const error = closeCode === 1009 ? oversized() : protocolError("terminal_protocol_error", closeCode);
    // The codec has already queued the protocol close. Retire the handle now
    // and let the peer receive the close within the bounded close handshake.
    settle(connection, error);
    notifyClose(connection, error);
    connection.onEnd?.(error);
  }

  function onError(connection, error) {
    if (connection.done) return;
    if (connection.opened && error.code?.startsWith("WS_ERR_")) {
      onCodecError(connection, error.code);
      return;
    }
    const code = connection.upgradeSeen && !connection.opened ? "terminal_protocol_error" : "terminal_network_error";
    end(connection, transportError(code));
  }

  function onOpen(connection, deadline) {
    try {
      ensure(connection);
      if (Date.now() >= deadline) fail("terminal_ticket_expired");
      clearTimeout(connection.handshakeTimer);
      connection.opened = true;
      connection.connected.resolve();
    } catch (error) {
      end(connection, error, { notify: false });
    }
  }

  function bindSocket(connection, socket, deadline) {
    socket.once("upgrade", (response) => {
      connection.upgradeSeen = true;
      if (!validUpgrade(response)) end(connection, protocolError());
    });
    socket.once("unexpected-response", (_request, response) => {
      end(connection, refusal(response.statusCode, "terminal_upgrade_refused"));
    });
    socket.once("open", () => onOpen(connection, deadline));
    socket.on("message", (data, binary) => onMessage(connection, data, binary));
    socket.on("error", (error) => onError(connection, error));
    socket.once("close", (closeCode, reason) => {
      unlink(connection);
      end(connection, Object.assign(protocolError("terminal_closed", closeCode), { reason: reason.toString() }));
    });
  }

  async function connect(connection, ticketBody) {
    const ticket = await issueTicket(device, connection, hostedOrigin, ticketBody);
    ensure(connection);
    const deadline = ticketDeadline(ticket, connection.generation);
    connection.socket = websocket(connection.handle.mint.origin, hostedOrigin, ticket.ticket, deadline);
    connection.connected = Promise.withResolvers();
    bindSocket(connection, connection.socket, deadline);
    connection.handshakeTimer = setTimeout(
      () => end(connection, transportError("terminal_ticket_expired")),
      Math.max(1, deadline - Date.now()),
    );
    await connection.connected.promise;
    ensure(connection);
    return { terminal: connection.id };
  }

  async function open(owner, id, onEvent, { signal, ticketBody, forwardReplies, onEnd } = {}) {
    const handle = handleFor(owner, id);
    const opening = handle.opening;
    if (!opening) fail("not_open");
    const mode = terminalMode(opening);
    if (mode !== "wss") fail(mode === "invalid" ? "terminal_protocol_error" : "terminal_unsupported");
    if (handle.mint.scope !== "bridge:read bridge:write") fail("terminal_forbidden");
    if (typeof onEvent !== "function") fail("invalid_params");
    if (signal?.aborted) fail("canceled");
    const body = ticketBody ?? { opening_generation: opening.opening_generation };
    if (body.opening_generation !== opening.opening_generation) fail("binding_mismatch");
    if (handle.terminal || handle.terminalRenewing) fail("terminal_busy");
    const connection = {
      id: randomUUID(), handle, onEvent, onEnd, forwardReplies, signal, abort: new AbortController(),
      generation: opening.opening_generation, bridgeEpoch: opening.greeting.bridge_epoch,
      calls: new Map(), writes: new Set(),
    };
    handle.terminal = connection;
    terminals.set(connection.id, connection);
    connection.cancel = () => end(connection, transportError("canceled"), { notify: false });
    signal?.addEventListener("abort", connection.cancel, { once: true });
    try {
      return await connect(connection, body);
    } catch (error) {
      end(connection, error, { notify: false });
      throw error;
    }
  }

  function current(owner, id) {
    const connection = terminals.get(id);
    if (!connection || connection.handle.owner !== owner) fail("invalid_terminal_handle");
    ensure(connection);
    if (!connection.opened) fail("terminal_closed");
    return connection;
  }

  function writeError(connection) {
    const error = transportError("terminal_network_error");
    end(connection, error, { notify: observable(connection) });
  }

  function send(connection, bytes) {
    const { socket, writes } = connection;
    if (socket.readyState !== WebSocket.OPEN) fail("terminal_closed");
    if (writes.size >= MAX_QUEUED_WRITES || socket.bufferedAmount + bytes.length > MAX_JSON_BYTES) {
      const error = transportError("terminal_queue_full");
      end(connection, error);
      throw error;
    }
    const timeout = setTimeout(() => writeError(connection), STALL_MS);
    writes.add(timeout);
    socket.send(bytes, { binary: true }, (error) => {
      clearTimeout(timeout);
      writes.delete(timeout);
      if (error) writeError(connection);
    });
  }

  async function call(owner, terminal, method, params, id = randomUUID()) {
    const connection = current(owner, terminal);
    validateCall(id, method, params);
    if (connection.calls.has(id)) fail("duplicate_request");
    if (connection.calls.size >= MAX_CALLS) fail("busy");
    const bytes = encode({ id, method, params });
    const pending = connection.forwardReplies ? null : Promise.withResolvers();
    send(connection, bytes);
    connection.calls.set(id, { ...pending, method });
    return pending?.promise;
  }

  function detach(owner, terminal, id) {
    const connection = current(owner, terminal);
    if (!wireId(id)) fail("invalid_params");
    send(connection, encode({ attach_id: id, type: "detach" }));
  }

  function close(owner, terminal) {
    const connection = current(owner, terminal);
    end(connection, transportError("terminal_closed"), { notify: false });
  }

  return { open, call, detach, close, clear };
}
