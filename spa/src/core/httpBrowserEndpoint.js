import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";
import { boundedJson, httpRequest, jsonHeaders } from "./httpFetch.js";
import { readTransientEvents } from "./httpSse.js";
import { openTerminalWebSocket } from "./terminalWebSocket.js";

const LIFECYCLE_BYTES = 64 * 1024;
const RESPONSE_BYTES = 8 * 1024 * 1024;
const REQUEST_BYTES = 1024 * 1024;
const segment = (value) => encodeURIComponent(value);
const sameOpening = (held, current) => held?.session_id === current?.session_id && held?.opening_generation === current?.opening_generation;
const withBody = (encoded) => encoded === undefined ? {} : { body: encoded };

export function createBrowserHttpEndpoint({
  grant, fetchImpl = fetch, isCurrent = () => true, now = Date.now, monotonic = () => performance.now(),
  callerOrigin = globalThis.location?.origin,
  createWebSocket = (url, protocols) => new WebSocket(url, protocols),
}) {
  const origin = canonicalBridgeOrigin(grant.identity.origin);
  const lifetime = new AbortController();
  let closed = false;
  let opening = null;
  let stream = null;
  let terminal = null;
  let onLost = () => {};
  let hardTimer;
  let refreshTimer;
  let hardDeadline = Infinity;
  const active = () => !closed && isCurrent() && monotonic() < hardDeadline;
  const check = () => { if (!active()) throw httpFailure("scope_ended"); };

  const headers = (extra = {}, hasBody = true) => {
    const value = { ...jsonHeaders, Authorization: `Bearer ${grant.token()}`, ...extra };
    if (!hasBody) delete value["Content-Type"];
    return value;
  };
  const deleteQuietly = (path, extra) => {
    try {
      const heldHeaders = headers(extra, false);
      void fetchImpl(origin + path, { method: "DELETE", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer",
        headers: heldHeaders, signal: AbortSignal.timeout(1000), cache: "no-store" }).catch(() => {});
    } catch { /* Teardown cannot restore expired authority. */ }
  };

  const close = () => {
    if (closed) return;
    closed = true;
    clearTimeout(hardTimer); clearTimeout(refreshTimer);
    terminal?.retire();
    stream?.close();
    if (opening) deleteQuietly(`/v1/sessions/${segment(opening.session_id)}`, {});
    lifetime.abort(httpFailure("scope_ended"));
    grant.close();
  };
  const lose = (error = httpFailure("authentication")) => {
    if (closed) return;
    close(); onLost(error);
  };

  const readResponse = async (response, maxBytes) => {
    if (response.status === 401) { lose(); throw httpFailure("authentication"); }
    const value = await boundedJson(response, maxBytes);
    if (!response.ok && typeof value?.error !== "string") throw httpFailure("protocol");
    return value;
  };
  const json = (path, method, body, options = {}) => {
    const { signal, timeoutMs, maxBytes, extra, requestMaxBytes, read } = {
      timeoutMs: 12000, maxBytes: LIFECYCLE_BYTES, extra: {}, requestMaxBytes: REQUEST_BYTES, read: readResponse, ...options,
    };
    check();
    if (!path.startsWith("/v1/") || /[?#]/.test(path)) throw httpFailure("protocol");
    const encoded = body === undefined ? undefined : JSON.stringify(body);
    if (encoded && new TextEncoder().encode(encoded).byteLength > requestMaxBytes) throw httpFailure("protocol");
    return httpRequest(async (requestSignal) => {
      let response;
      try {
        response = await fetchImpl(origin + path, { method, credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", cache: "no-store",
          headers: headers(extra, encoded !== undefined), signal: requestSignal, ...withBody(encoded) });
      } catch { throw httpFailure("network", true); }
      return read(response, maxBytes);
    }, { signal: signal ? AbortSignal.any([lifetime.signal, signal]) : lifetime.signal, timeoutMs, isCurrent: active });
  };

  const armGrant = (value, started) => {
    const serverDeadline = Date.parse(value.grant_expires_at);
    const wait = Math.min(30000 - (monotonic() - started), serverDeadline - now(), grant.expiresAt() - now());
    if (!Number.isFinite(wait) || wait <= 0) throw httpFailure("authentication");
    hardDeadline = monotonic() + wait;
    clearTimeout(hardTimer); clearTimeout(refreshTimer);
    hardTimer = setTimeout(() => lose(), wait);
    refreshTimer = setTimeout(() => { void refresh().catch(lose); }, Math.min(20000, wait * 2 / 3));
  };
  const refresh = async () => {
    check();
    if (grant.expiresAt() - now() < 60000) {
      const previous = grant.token();
      await grant.renew();
      if (grant.token() !== previous) terminal?.retire(rotatedTerminalCredentials());
    }
    check();
    const started = monotonic();
    const renewed = await json(`/v1/sessions/${segment(opening.session_id)}`, "PATCH", undefined, { timeoutMs: 5000 });
    assertLifecycle(renewed);
    if (renewed.session_id !== opening.session_id || renewed.opening_generation !== opening.opening_generation) throw httpFailure("protocol");
    armGrant(renewed, started);
  };

  const subscribe = async (heldOpening, consume) => {
    check();
    if (stream || !sameOpening(heldOpening, opening)) throw httpFailure("protocol");
    const binding = await json("/v1/subscriptions", "POST", { kind: "application" }, { extra: { "X-Build-Session": opening.session_id } });
    assertLifecycle(binding);
    const path = `/v1/subscriptions/${segment(binding.subscription_id)}/events`;
    if (typeof binding.subscription_id !== "string" || binding.opening_generation !== opening.opening_generation || binding.events_path !== path) throw httpFailure("protocol");
    const controller = new AbortController();
    const signal = AbortSignal.any([lifetime.signal, controller.signal]);
    let idleTimer;
    let resolveReady; let rejectReady;
    const ready = new Promise((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
    const activity = () => { clearTimeout(idleTimer); idleTimer = setTimeout(() => lose(httpFailure("stream_reset")), 45000); };
    const stop = () => {
      controller.abort(); clearTimeout(idleTimer);
      deleteQuietly(`/v1/subscriptions/${segment(binding.subscription_id)}`, { "X-Build-Session": opening.session_id });
    };
    stream = { binding: binding.subscription_id, close: stop };
    const read = async () => {
      const response = await fetchImpl(origin + path, { method: "GET", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", cache: "no-store", signal,
        headers: headers({ Accept: "text/event-stream", "X-Build-Session": opening.session_id }, false) });
      activity();
      await readTransientEvents(response, async (event) => {
        check();
        if (event.event === "hello" && event.data?.subscription_id !== binding.subscription_id) throw httpFailure("protocol");
        await consume(event);
        if (event.event === "hello") resolveReady();
      }, activity);
      throw httpFailure("stream_reset");
    };
    void read().catch((error) => { rejectReady(error); if (active()) lose(httpFailure("stream_reset")); });
    try {
      await httpRequest(() => ready, { signal, timeoutMs: 5000, isCurrent: active });
      return stream;
    } catch (error) { stop(); throw error; }
  };

  const openTerminal = async (heldOpening, { signal } = {}) => {
    check();
    if (!opening || !sameOpening(heldOpening, opening) || signal?.aborted) throw httpFailure("scope_ended");
    const url = terminalUrl(origin, callerOrigin);
    if (terminal && !terminal.signal.aborted) return terminal.ready;

    const controller = new AbortController();
    const terminalSignal = signal ? AbortSignal.any([controller.signal, signal]) : controller.signal;
    const attempt = {
      signal: terminalSignal,
      retire: (error = httpFailure("scope_ended")) => controller.abort(error),
    };
    const release = () => { if (terminal === attempt) terminal = null; };
    const { session_id, opening_generation } = opening;
    const epoch = terminalEpoch(opening);
    const issueTicket = (signal) => json("/v1/terminal-tickets", "POST", { opening_generation }, {
      signal, extra: { "X-Build-Session": session_id },
      read: (response, maxBytes) => readTerminalTicket(response, maxBytes, opening_generation, now),
    });
    terminal = attempt;
    attempt.ready = openTerminalWebSocket({
      url, issueTicket, createWebSocket, epoch, now,
      signal: terminalSignal, isCurrent: () => active() && terminal === attempt, onEnd: release,
      retiredError: () => controller.signal.reason || httpFailure("scope_ended"),
    }).catch((error) => { release(); throw error; });
    return attempt.ready;
  };

  return {
    identity: grant.identity,
    onLost: (listener) => { onLost = listener; },
    async open(apiRange) {
      if (opening) throw httpFailure("protocol");
      const started = monotonic();
      opening = await json("/v1/sessions", "POST", { api_range: apiRange });
      assertLifecycle(opening);
      armGrant(opening, started);
      return opening;
    },
    async request(heldOpening, route, options) {
      check();
      if (!sameOpening(heldOpening, opening)) throw httpFailure("scope_ended");
      const extra = resourceHeaders(opening, options);
      return json(route.path, route.method, route.body, { ...options, extra, maxBytes: route.responseMaxBytes || RESPONSE_BYTES, requestMaxBytes: route.requestMaxBytes || REQUEST_BYTES });
    },
    subscribe, openTerminal, close,
  };
}

function assertLifecycle(value) {
  if (typeof value?.error !== "string") return;
  const code = value.error_code === "unsupported_version" ? "unsupported" : value.error_code;
  throw Object.assign(httpFailure(code), { bridgeRefusal: true, retryable: value.retryable === true });
}

function resourceHeaders(opening, options) {
  const extra = {
    "X-Build-Session": opening.session_id, "X-Build-Request-Id": options.requestId || crypto.randomUUID(),
    "X-Build-Priority": options.priority === "background" ? "background" : "user",
  };
  if (options.subscriptionId) extra["X-Build-Subscription"] = options.subscriptionId;
  if (options.write) {
    extra["Idempotency-Key"] = options.write.operation_id;
    if (options.write.after !== undefined) extra["X-Build-After"] = JSON.stringify(options.write.after);
  }
  return extra;
}

function terminalEpoch(opening) {
  const epoch = opening.greeting?.bridge_epoch;
  if (typeof epoch !== "string" || !epoch) throw httpFailure("protocol");
  return epoch;
}

function terminalUrl(origin, callerOrigin) {
  let caller;
  try { caller = new URL(canonicalBridgeOrigin(callerOrigin)); } catch { throw forbiddenTerminalOrigin(); }
  const url = new URL("/v1/terminal", origin);
  if (caller.hostname === url.hostname) throw forbiddenTerminalOrigin();
  url.protocol = "wss:";
  return url.href;
}

function forbiddenTerminalOrigin() {
  return Object.assign(httpFailure("origin_forbidden"), {
    message: "Browser terminals require the app and HTTPS bridge to use different hostnames.", retryable: false,
  });
}

// 32 bytes leave two zero pad bits in the final base64url character.
const terminalTicket = /^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/u;
const ticketRefusals = new Set(["invalid_credential", "forbidden_scope", "overloaded", "origin_forbidden", "authority_mismatch"]);
const utcTimestamp = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/u;

async function readTerminalTicket(response, maxBytes, generation, now) {
  const body = await boundedJson(response, maxBytes);
  if (response.status !== 201) throw ticketFailure(response.status, body);
  const expiresAt = ticketExpiry(body?.expires_at);
  if (!(expiresAt > now()) || body.opening_generation !== generation
    || typeof body.ticket !== "string" || !terminalTicket.test(body.ticket)) throw httpFailure("protocol");
  return { token: body.ticket, expiresAt };
}

function ticketExpiry(timestamp) {
  if (typeof timestamp !== "string" || !utcTimestamp.test(timestamp)) return NaN;
  return Date.parse(timestamp);
}

function ticketFailure(status, body) {
  if (status >= 200 && status < 300) return httpFailure("protocol");
  const code = ticketRefusals.has(body?.error_code) ? body.error_code : "protocol";
  const error = httpFailure(code);
  error.message = ticketFailureMessage(status, code) || error.message;
  error.httpStatus = status;
  error.retryable = (status === 429 || status >= 500) && body?.retryable === true;
  return error;
}

function ticketFailureMessage(status, code) {
  if (status === 429) return "Terminal connection limit reached";
  if (status === 401) return httpFailure("authentication").message;
  if (code === "forbidden_scope") return "This device grant does not allow terminal access.";
}

function rotatedTerminalCredentials() {
  return Object.assign(new Error("Terminal credentials changed."), {
    code: "credentials_rotated", error_code: "credentials_rotated", retryable: true, uncertain: false,
  });
}
