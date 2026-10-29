import { SPA_API_RANGE } from "./bridgeApi/index.js";
import { createBridgeEpoch, endedOpeningError } from "./bridgeEpoch.js";
import { httpFailure } from "./httpEndpoint.js";
import { httpRequest } from "./httpFetch.js";

const optionsOf = (options) => typeof options === "number" || options === null ? { timeoutMs: options } : options;
const handle = (value) => typeof value === "string" && value.length > 0 && value.length <= 128;

export function validateHttpOpening(opening, routes) {
  validateTransport(opening?.transport, routes);
  if (!handle(opening.session_id) || !handle(opening.opening_generation) || !opening.greeting) throw httpFailure("protocol");
}

function validateTransport(transport, routes) {
  if (transport?.resource_manifest_version !== 1 || transport.terminal !== "webrtc" || transport.durable_events !== false) throw httpFailure("unsupported");
  if (!Array.isArray(transport.operation_ids) || !routes.operationIds().every((id) => transport.operation_ids.includes(id))) throw httpFailure("unsupported");
}

const validHello = (data, opening) => data?.durable === false && data.recovery === "current_state_refresh"
  && data.opening_generation === opening.opening_generation && data.bridge_epoch === opening.greeting.bridge_epoch;

function refusal(frame) {
  return Object.assign(new Error(frame.error), {
    error_code: frame.error_code, retryable: frame.retryable === true, details: frame.details, uncertain: false, bridgeRefusal: true,
  });
}

export async function openHttpSession({ endpoint, routes, onPush = () => {}, onLost = () => {}, isCurrent = () => true }) {
  let closed = false;
  let subscription;
  let opening;
  let adapter = null;
  let hello = false;
  let gateRelease;
  const gate = new Promise((resolve) => { gateRelease = resolve; });
  let refreshRelease;
  const refreshGate = new Promise((resolve) => { refreshRelease = resolve; });
  const controllers = new Set();
  const pushListeners = new Set([onPush]);
  const writeListeners = new Set();
  const closeListeners = new Set();
  const bridgeEpoch = createBridgeEpoch();
  const current = () => !closed && isCurrent();
  const check = () => { if (!current()) throw httpFailure("scope_ended"); };

  const close = () => {
    if (closed) return;
    closed = true;
    gateRelease();
    refreshRelease();
    for (const controller of controllers) controller.abort(httpFailure("scope_ended"));
    controllers.clear();
    subscription?.close();
    endpoint.close();
    for (const listener of [...closeListeners]) { try { listener(); } catch { /* Retire every owned resource. */ } }
    closeListeners.clear();
    pushListeners.clear(); writeListeners.clear();
  };
  const lose = (error = httpFailure("stream_reset")) => {
    if (closed) return;
    close();
    onLost(error);
  };
  const take = async ({ event, data }) => {
    if (!current()) return;
    if (event === "hello") {
      if (hello || !validHello(data, opening)) {
        lose(httpFailure("protocol")); throw httpFailure("protocol");
      }
      hello = true;
      return;
    }
    if (!hello) { lose(httpFailure("protocol")); throw httpFailure("protocol"); }
    if (["reset_required", "closing"].includes(event)) { lose(); return; }
    if (!["changes", "operation", "event"].includes(event)) return;
    if (!bridgeEpoch.accepts(data)) { lose(); return; }
    publishWrite(data);
    for (const listener of [...pushListeners]) await listener(data);
  };
  const publishWrite = (frame) => {
    if (frame.type !== "writes.confirmed" && frame.write?.phase !== "ack") return;
    for (const listener of [...writeListeners]) listener(frame);
  };

  try {
    opening = await endpoint.open(SPA_API_RANGE);
    check(); validateHttpOpening(opening, routes);
    bridgeEpoch.acceptGreeting(opening.greeting, { isCurrent: current });
    endpoint.onLost?.(lose);
    subscription = await endpoint.subscribe(opening, take);
    check();
    if (!hello) throw httpFailure("protocol");
  } catch (error) {
    close(); throw error;
  }

  const awaitRefresh = async (method, options) => {
    if (!["ping", "changes.subscribe", "changes.unsubscribe"].includes(method)) {
      await httpRequest(() => refreshGate, { signal: optionsOf(options)?.signal, timeoutMs: 0, isCurrent: current });
      check();
    }
  };
  const rawCall = async (method, params = {}, options = {}) => {
    check();
    if (method === "session.hello") return opening.greeting;
    await awaitRefresh(method, options);
    const route = routes.request(method, params);
    const heldOptions = optionsOf(options) || {};
    const timeoutMs = Object.hasOwn(heldOptions, "timeoutMs") ? heldOptions.timeoutMs : defaultTimeout(method);
    const assertCurrent = () => { check(); if (heldOptions.assertCurrent) heldOptions.assertCurrent(); };
    assertCurrent();
    if (controllers.size >= 128) throw Object.assign(httpFailure("busy"), { retryable: true });
    const controller = new AbortController();
    controllers.add(controller);
    const signal = heldOptions.signal ? AbortSignal.any([controller.signal, heldOptions.signal]) : controller.signal;
    let sent = false;
    try {
      return await httpRequest(async (requestSignal) => {
        assertCurrent();
        sent = true;
        const frame = await endpoint.request(opening, route, { ...heldOptions, timeoutMs, signal: requestSignal, subscriptionId: subscription.binding });
        assertCurrent();
        if (!bridgeEpoch.accepts(frame)) { lose(); throw endedOpeningError(method, true); }
        if (typeof frame.error === "string") throw refusal(frame);
        if (frame.ok !== true) throw httpFailure("protocol", true);
        publishWrite(frame);
        return heldOptions.write ? frame : frame.result;
      }, { signal, timeoutMs, isCurrent: current });
    } catch (error) {
      if (!error.bridgeRefusal) error.uncertain = sent;
      throw error;
    } finally {
      controllers.delete(controller);
    }
  };

  return {
    deviceId: endpoint.identity.deviceId, sessionId: opening.session_id, transport: "http", terminalTransport: "webrtc",
    bridgeEpoch, rawCall,
    allowRefresh: refreshRelease,
    onClose(fn) { if (closed) fn(); else closeListeners.add(fn); return () => closeListeners.delete(fn); },
    async call(method, params = {}, options = {}) {
      if (method === "session.hello" || method === "ping") return rawCall(method, params, options);
      await httpRequest(() => gate, { signal: optionsOf(options)?.signal, timeoutMs: 0, isCurrent: current });
      check();
      if (!adapter) throw httpFailure("unsupported");
      return adapter.call(method, params, options);
    },
    installAdapter(selection, options) {
      adapter = selection && !selection.unsupported ? selection.create(rawCall, options) : null;
      gateRelease(); return adapter;
    },
    adapter: () => adapter,
    acceptGreeting: (greeting, options) => bridgeEpoch.acceptGreeting(greeting, options),
    onPush: (fn) => { pushListeners.add(fn); return () => pushListeners.delete(fn); },
    onWriteFrame: (fn) => { writeListeners.add(fn); return () => writeListeners.delete(fn); },
    fail: close, close,
  };
}

const defaultTimeout = (method) => method === "workspace.get" ? null : 12000;
