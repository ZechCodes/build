import { randomUUID } from "node:crypto";
import { terminalIpcArguments, terminalRequestSequence } from "./bridge-http-contract.mjs";
import { createBridgeHttp } from "./bridge-http.mjs";
import { transportError, MAX_JSON_BYTES } from "./bridge-http-network.mjs";
import { APP_URL } from "./security-policy.mjs";

const CHANNEL = "build:bridge-http";
const EVENT = "build:bridge-http-event";
const TERMINAL_EVENT = "build:bridge-http-terminal-event";
const MAX_TERMINAL_ARGS = 128 * 1024;
const TERMINAL_ARITY = new Map([
  ["terminalOpen", [3, 3]],
  ["terminalCall", [4, 4]],
  ["terminalConsume", [2, 2]],
  ["terminalDetach", [2, 2]],
  ["terminalClose", [1, 1]],
]);
const ARITY = new Map([
  ["discover", [1, 1]],
  ["discoverSaved", [2, 2]],
  ["settingsRead", [0, 0]],
  ["settingsWrite", [1, 1]],
  ["hostedNotice", [1, 1]],
  ["resume", [1, 1]],
  ["open", [2, 2]],
  ["request", [3, 4]],
  ["subscribe", [3, 3]],
  ["ack", [2, 2]],
  ["unsubscribe", [1, 1]],
  ["cancel", [1, 1]],
  ["close", [1, 1]],
]);
const { origin, pathname } = new URL(APP_URL);
function trustedUrl(value) {
  try {
    const url = new URL(value);
    return url.origin === origin && url.pathname === pathname;
  } catch {
    return false;
  }
}

function safeError(error, terminal) {
  const safe = !terminal && error?.code === "discovery_failed"
    ? transportError("discovery_failed")
    : error?.message?.startsWith("Bridge HTTP ")
    ? error
    : transportError("request_failed");
  return {
    ok: false,
    error: {
      code: safe.code,
      message: safe.message,
      ...(error?.uncertain ? { uncertain: true } : {}),
      ...(terminal && Number.isInteger(safe.status) && { status: safe.status }),
      ...(terminal && typeof safe.retryable === "boolean" && { retryable: safe.retryable }),
    },
  };
}
function validateArguments(method, args, terminal) {
  const limits = (terminal ? TERMINAL_ARITY : ARITY).get(method);
  if (!limits) throw transportError("unsupported_operation");
  if (
    !Array.isArray(args) ||
    args.length < limits[0] ||
    args.length > limits[1]
  )
    throw transportError("invalid_params");
  if (terminal) terminalIpcArguments(method, args);
  let bytes;
  try { bytes = Buffer.byteLength(JSON.stringify(args)); }
  catch { throw transportError("invalid_params"); }
  if (bytes > (terminal ? MAX_TERMINAL_ARGS : 8 * MAX_JSON_BYTES))
    throw transportError("invalid_params");
}

export function installBridgeHttpIpc({
  ipcMain,
  getWebContents,
  getAuth,
  fetch,
  manifest,
  settings,
  getSettingsToken = () => getAuth()?.accessToken,
}) {
  const owners = new WeakMap();
  const navigations = new WeakMap();
  let nextId = 0;
  const isOwnerCurrent = (owner) =>
    owners.get(owner.contents) === owner &&
    getWebContents() === owner.contents &&
    owner.contents.mainFrame === owner.frame &&
    trustedUrl(owner.frame.url);
  const bridge = createBridgeHttp({
    fetch,
    manifest,
    getAuth,
    isOwnerCurrent,
    send: (owner, event) => {
      if (isOwnerCurrent(owner)) owner.contents.send(EVENT, event);
    },
  });
  const terminal = createTerminalIpc({
    bridge, getAuth,
    isOwnerCurrent: (owner) => isOwnerCurrent(owner) && !navigationPending(owner.contents),
  });
  function retire(contents) {
    const owner = owners.get(contents);
    if (owner) {
      owners.delete(contents);
      bridge.clear(owner);
    }
  }
  function navigationPending(contents) {
    const navigation = navigations.get(contents);
    if (!navigation) return false;
    if (!navigation.checkIdle || contents.isLoadingMainFrame?.() !== false || contents.isWaitingForResponse?.())
      return true;
    navigations.delete(contents);
    return false;
  }
  function checkNavigationAfterEvent(contents) {
    const navigation = navigations.get(contents);
    if (!navigation) return;
    // Electron may emit start, cancellation or failure before Chromium
    // updates loading state. Uncommitted requests can finish without any
    // commit or failure event. Only the current start may release its fence.
    setImmediate(() => {
      if (navigations.get(contents) !== navigation) return;
      navigation.checkIdle = true;
      navigationPending(contents);
    });
  }
  function attach(contents) {
    contents.on(
      "did-start-navigation",
      (details, _url, inPlace, isMainFrame) => {
        if ((details.isMainFrame ?? isMainFrame) && !(details.isSameDocument ?? inPlace)) {
          navigations.set(contents, { checkIdle: false });
          retire(contents);
          checkNavigationAfterEvent(contents);
        }
      },
    );
    contents.on("did-navigate", () => {
      // Main-frame commit changes document authority, including when the
      // frame is reused. A newer outstanding request still owns its fence.
      retire(contents);
      if (!contents.isWaitingForResponse?.()) navigations.delete(contents);
    });
    const checkFailure = (_event, _code, _description, _url, isMainFrame) => {
      if (isMainFrame) checkNavigationAfterEvent(contents);
    };
    contents.on("did-fail-provisional-load", checkFailure);
    contents.on("did-fail-load", checkFailure);
    contents.on("did-stop-loading", () => checkNavigationAfterEvent(contents));
    const discard = () => { navigations.delete(contents); retire(contents); };
    contents.on("render-process-gone", discard);
    contents.on("destroyed", discard);
  }
  function caller(event) {
    const contents = getWebContents();
    if (
      !contents ||
      event.sender !== contents ||
      event.senderFrame !== contents.mainFrame ||
      !trustedUrl(event.senderFrame?.url)
    )
      throw transportError("untrusted_caller");
    let owner = owners.get(contents);
    if (owner && owner.frame !== event.senderFrame) { retire(contents); owner = undefined; }
    if (!owner) {
      owner = { contents, frame: event.senderFrame, id: ++nextId };
      owners.set(contents, owner);
    }
    return owner;
  }
  ipcMain.handle(CHANNEL, async (event, method, args) => {
    const isTerminal = TERMINAL_ARITY.has(method);
    try {
      if (isTerminal && navigationPending(event.sender)) throw transportError("untrusted_caller");
      const owner = caller(event);
      validateArguments(method, args, isTerminal);
      const value = await invoke(owner, method, args, isTerminal);
      if (!isOwnerCurrent(owner)) throw transportError("stale_owner");
      return { ok: true, value };
    } catch (error) {
      return safeError(error, isTerminal);
    }
  });
  function invoke(owner, method, args, isTerminal) {
    if (method === "settingsRead") return settings.read(getSettingsToken());
    if (method === "settingsWrite") return settings.write(getSettingsToken(), args[0]);
    if (method === "hostedNotice") return settings.hostedNotice(getSettingsToken(), args[0]);
    return (isTerminal ? terminal : bridge)[method](owner, ...args);
  }
  return {
    attach,
    retire,
    clear: () => bridge.clear(),
    revoke: (token) => bridge.revoke(token),
  };
}

const MAX_DELIVERIES = 128;
const MAX_ORDINARY_DELIVERY_BYTES = 2 * MAX_JSON_BYTES;
const MAX_SNAPSHOT_DELIVERIES = 2;
const DELIVERY_TIMEOUT = 10_000;

function createTerminalIpc({ bridge, getAuth, isOwnerCurrent }) {
  const active = new Map();
  const opening = new Map();

  function release(binding) {
    active.delete(binding.id);
    if (opening.get(binding.requestId) === binding) opening.delete(binding.requestId);
    clearTimeout(binding.timer);
    binding.receipts.clear();
  }

  function close(binding) {
    release(binding);
    binding.abort.abort();
  }

  function authorized(binding) {
    const auth = getAuth();
    return Boolean(auth?.accessToken) && auth.generation === binding.generation &&
      isOwnerCurrent(binding.owner);
  }

  function find(owner, binding) {
    if (!binding || binding.owner !== owner) throw transportError("invalid_terminal_handle");
    if (!authorized(binding)) {
      close(binding);
      throw transportError("stale_owner");
    }
    return binding;
  }

  function send(binding, event) {
    if (active.get(binding.id) !== binding || !authorized(binding)) return;
    try {
      binding.owner.frame.send(TERMINAL_EVENT, {
        terminal: binding.id, requestId: binding.requestId, ...event,
      });
    } catch {
      close(binding);
    }
  }

  function failDelivery(binding, code) {
    send(binding, { type: "close", code });
    close(binding);
  }

  function watchDelivery(binding) {
    clearTimeout(binding.timer);
    const oldest = binding.receipts.values().next().value;
    if (!oldest) return;
    binding.timer = setTimeout(
      () => failDelivery(binding, "terminal_delivery_timeout"),
      Math.max(0, oldest.deadline - performance.now()),
    );
    binding.timer.unref?.();
  }

  function deliver(binding, event, delivery) {
    if (event.type === "close") {
      send(binding, event);
      return;
    }
    const { bytes, snapshot } = delivery;
    const large = snapshot && bytes > MAX_JSON_BYTES;
    const ordinary = large ? 0 : bytes;
    if (binding.receipts.size >= MAX_DELIVERIES ||
        binding.bytes + ordinary > MAX_ORDINARY_DELIVERY_BYTES ||
        binding.snapshots + Number(large) > MAX_SNAPSHOT_DELIVERIES) {
      failDelivery(binding, "terminal_delivery_overflow");
      return;
    }
    const sequence = ++binding.sequence;
    binding.bytes += ordinary;
    binding.snapshots += Number(large);
    binding.receipts.set(sequence, { ordinary, large, deadline: performance.now() + DELIVERY_TIMEOUT });
    if (binding.receipts.size === 1) watchDelivery(binding);
    send(binding, { ...event, sequence });
  }

  return {
    async terminalOpen(owner, handle, requestId, ticketBody) {
      if (opening.has(requestId)) throw transportError("duplicate_request");
      const binding = {
        owner, requestId, id: randomUUID(), generation: getAuth()?.generation,
        abort: new AbortController(), receipts: new Map(), sequence: 0,
        bytes: 0, snapshots: 0, lastRequest: 0,
      };
      active.set(binding.id, binding);
      opening.set(requestId, binding);
      try {
        const result = await bridge.terminalOpen(owner, handle, ({ terminal: _handle, ...event }, delivery) =>
          deliver(binding, event, delivery), {
          signal: binding.abort.signal, ticketBody, forwardReplies: true, onEnd: () => release(binding),
        });
        find(owner, active.get(binding.id));
        binding.native = result.terminal;
        return { terminal: binding.id };
      } catch (error) {
        close(binding);
        throw error;
      } finally {
        if (opening.get(requestId) === binding) opening.delete(requestId);
      }
    },
    terminalCall(owner, terminal, method, params, id) {
      const binding = find(owner, active.get(terminal));
      const request = terminalRequestSequence(id);
      if (!binding.native || request <= binding.lastRequest) throw transportError("invalid_params");
      binding.lastRequest = request;
      return bridge.terminalCall(owner, binding.native, method, params, id);
    },
    terminalDetach(owner, terminal, id) {
      const binding = find(owner, active.get(terminal));
      if (!binding.native || terminalRequestSequence(id) > binding.lastRequest) throw transportError("invalid_params");
      return bridge.terminalDetach(owner, binding.native, id);
    },
    terminalConsume(owner, terminal, sequence) {
      const binding = find(owner, active.get(terminal));
      if (sequence !== binding.receipts.keys().next().value) throw transportError("invalid_params");
      const receipt = binding.receipts.get(sequence);
      binding.receipts.delete(sequence);
      binding.bytes -= receipt.ordinary;
      binding.snapshots -= Number(receipt.large);
      watchDelivery(binding);
    },
    terminalClose(owner, id) {
      close(find(owner, active.get(id) ?? opening.get(id)));
    },
  };
}
