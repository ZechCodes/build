import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";
import { boundedJson, httpRequest, jsonHeaders } from "./httpFetch.js";
import { readTransientEvents } from "./httpSse.js";

const LIFECYCLE_BYTES = 64 * 1024;
const RESPONSE_BYTES = 8 * 1024 * 1024;
const REQUEST_BYTES = 1024 * 1024;
const segment = (value) => encodeURIComponent(value);
const sameOpening = (held, current) => held?.session_id === current?.session_id && held?.opening_generation === current?.opening_generation;
const withBody = (encoded) => encoded === undefined ? {} : { body: encoded };

export function createBrowserHttpEndpoint({ grant, fetchImpl = fetch, isCurrent = () => true, now = Date.now, monotonic = () => performance.now() }) {
  const origin = canonicalBridgeOrigin(grant.identity.origin);
  const lifetime = new AbortController();
  let closed = false;
  let opening = null;
  let stream = null;
  let onLost = () => {};
  let hardTimer;
  let refreshTimer;
  let hardDeadline = Infinity;
  const active = () => !closed && isCurrent() && monotonic() < hardDeadline;
  const check = () => { if (!active()) throw httpFailure("scope_ended"); };

  const headers = (extra = {}) => ({ ...jsonHeaders, Authorization: `Bearer ${grant.token()}`, ...extra });
  const deleteQuietly = (path, extra) => {
    try {
      const heldHeaders = headers(extra);
      void fetchImpl(origin + path, { method: "DELETE", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer",
        headers: heldHeaders, signal: AbortSignal.timeout(1000), cache: "no-store" }).catch(() => {});
    } catch { /* Teardown cannot restore expired authority. */ }
  };

  const close = () => {
    if (closed) return;
    closed = true;
    clearTimeout(hardTimer); clearTimeout(refreshTimer);
    stream?.close();
    if (opening) deleteQuietly(`/v1/sessions/${segment(opening.session_id)}`, {});
    lifetime.abort(httpFailure("scope_ended"));
    grant.close();
  };
  const lose = (error = httpFailure("authentication")) => {
    if (closed) return;
    close(); onLost(error);
  };

  const json = (path, method, body, options = {}) => {
    const { signal, timeoutMs, maxBytes, extra, requestMaxBytes } = { timeoutMs: 12000, maxBytes: LIFECYCLE_BYTES, extra: {}, requestMaxBytes: REQUEST_BYTES, ...options };
    check();
    if (!path.startsWith("/v1/") || /[?#]/.test(path)) throw httpFailure("protocol");
    const encoded = body === undefined ? undefined : JSON.stringify(body);
    if (encoded && new TextEncoder().encode(encoded).byteLength > requestMaxBytes) throw httpFailure("protocol");
    return httpRequest(async (requestSignal) => {
      let response;
      try {
        response = await fetchImpl(origin + path, { method, credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", cache: "no-store",
          headers: headers(extra), signal: requestSignal, ...withBody(encoded) });
      } catch { throw httpFailure("network", true); }
      if (response.status === 401 || response.status === 403) { lose(); throw httpFailure("authentication"); }
      const value = await boundedJson(response, maxBytes);
      if (!response.ok && typeof value?.error !== "string") throw httpFailure("protocol");
      return value;
    }, { signal: signal ? AbortSignal.any([lifetime.signal, signal]) : lifetime.signal, timeoutMs, isCurrent: active });
  };

  const armGrant = (value, started) => {
    const serverDeadline = Date.parse(value.grant_expires_at);
    const wait = Math.min(30000 - (monotonic() - started), serverDeadline - now(), grant.expiresAt() - now());
    if (!Number.isFinite(wait) || wait <= 0) throw httpFailure("authentication");
    hardDeadline = monotonic() + wait;
    clearTimeout(hardTimer); clearTimeout(refreshTimer);
    hardTimer = setTimeout(() => lose(), wait);
    refreshTimer = setTimeout(() => { void refresh().catch(() => lose()); }, Math.min(20000, wait * 2 / 3));
  };
  const refresh = async () => {
    check();
    if (grant.expiresAt() - now() < 60000) await grant.renew();
    check();
    const started = monotonic();
    const renewed = await json(`/v1/sessions/${segment(opening.session_id)}`, "PATCH", undefined, { timeoutMs: 5000 });
    if (renewed.session_id !== opening.session_id || renewed.opening_generation !== opening.opening_generation) throw httpFailure("protocol");
    armGrant(renewed, started);
  };

  const subscribe = async (heldOpening, consume) => {
    check();
    if (stream || !sameOpening(heldOpening, opening)) throw httpFailure("protocol");
    const binding = await json("/v1/subscriptions", "POST", { kind: "application" }, { extra: { "X-Build-Session": opening.session_id } });
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
        headers: headers({ Accept: "text/event-stream", "X-Build-Session": opening.session_id }) });
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

  return {
    identity: grant.identity,
    onLost: (listener) => { onLost = listener; },
    async open(apiRange) {
      if (opening) throw httpFailure("protocol");
      const started = monotonic();
      opening = await json("/v1/sessions", "POST", { api_range: apiRange });
      armGrant(opening, started);
      return opening;
    },
    async request(heldOpening, route, options) {
      check();
      if (!sameOpening(heldOpening, opening)) throw httpFailure("scope_ended");
      const extra = resourceHeaders(opening, options);
      return json(route.path, route.method, route.body, { ...options, extra, maxBytes: route.responseMaxBytes || RESPONSE_BYTES, requestMaxBytes: route.requestMaxBytes || REQUEST_BYTES });
    },
    subscribe, close,
  };
}

function resourceHeaders(opening, options) {
  const extra = {
    "X-Build-Session": opening.session_id, "X-Build-Request-Id": options.requestId || crypto.randomUUID(),
    "X-Build-Priority": options.priority === "background" ? "background" : "user",
    "X-Build-Subscription": options.subscriptionId,
  };
  if (options.write) {
    extra["Idempotency-Key"] = options.write.operation_id;
    if (options.write.after !== undefined) extra["X-Build-After"] = JSON.stringify(options.write.after);
  }
  return extra;
}
