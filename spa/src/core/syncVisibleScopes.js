// Mounted panes describe which cached records are on screen. The sync planner
// uses their addresses; read reports and unread displays share their presence.
// These getters describe local UI state and never read connection state.
import { readerIsHere } from "./readerPresence.js";

const trees = new Set();
const diffs = new Set();
const threads = new Set();
const reads = new Set();
const readerListeners = new Set();
let readerSignature = "";
const addressOf = (address) => typeof address === "function" ? address() : address;
const alwaysActive = () => true;

const register = (registry, address, value, active) => {
  let mounted = true;
  const scope = { address, value, active: () => mounted && active() };
  registry.add(scope);
  return () => { mounted = false; registry.delete(scope); };
};

export const registerSyncTree = (address, pathsGetter, activeGetter = alwaysActive) =>
  register(trees, address, pathsGetter, activeGetter);

export const registerSyncDiff = (address, activeGetter = alwaysActive) =>
  register(diffs, address, null, activeGetter);

export const registerSyncThread = (identityGetter, activeGetter = alwaysActive) => {
  const stop = register(threads, identityGetter, null, activeGetter);
  // A rail registers while its mount is still initializing its painters.
  queueMicrotask(notifyThreadReaders);
  return () => { stop(); notifyThreadReaders(); };
};

export const registerSyncRead = (keyGetter, read, activeGetter = alwaysActive) =>
  register(reads, keyGetter, read, activeGetter);

const readKey = (key) => JSON.stringify([key.deviceId, key.entityId, key.kind, key.sub || ""]);
const readMounted = (scope, key, passActive) => {
  const active = () => {
    const current = addressOf(scope.address);
    return scope.active() && passActive() && current && readKey(current) === readKey(key);
  };
  return active() ? scope.value(active) : null;
};

/** Source panes own reads that the board's entity sync cannot address. The
 *  scheduler awaits their normal fenced cache writes in the foreground phase. */
export function visibleSyncReads(deviceId) {
  const jobs = new Map();
  for (const scope of reads) {
    if (!scope.active()) continue;
    const key = addressOf(scope.address);
    if (key?.deviceId !== deviceId) continue;
    const identity = readKey(key);
    if (!jobs.has(identity)) jobs.set(identity, { key, read: (active = alwaysActive) => readMounted(scope, key, active) });
  }
  return [...jobs.values()];
}

/** The mounted chat may remember a different agent than the URL names. */
export function visibleSyncThread(deviceId) {
  for (const scope of threads) {
    if (!scope.active()) continue;
    const identity = addressOf(scope.address);
    if (identity?.deviceId === deviceId && identity.entityId && identity.agentId) return identity;
  }
  return null;
}

const presentThreads = () => {
  if (!readerIsHere()) return [];
  return [...threads].filter((scope) => scope.active()).map((scope) => addressOf(scope.address)).filter(Boolean);
};

const sameThreadGeneration = (a, b) => !a.threadId || !b.threadId || a.threadId === b.threadId;
const sameConversation = (a, b) => {
  if (!a || !b || a.deviceId !== b.deviceId || a.entityId !== b.entityId) return false;
  if (!sameThreadGeneration(a, b)) return false;
  return a.conversationId && b.conversationId
    ? a.conversationId === b.conversationId : !!a.agentId && a.agentId === b.agentId;
};

/** The read reporter and Inbox use the same mounted-chat and window-presence
 * signal. Canonical history identity survives agent aliases; devices and
 * conversation resets remain separate. No connection or RPC state is involved. */
export const threadReaderIsHere = (identity) =>
  presentThreads().some((shown) => sameConversation(shown, identity));

/** UI transitions repaint the held feed immediately, without rewriting the
 * bridge's unread count. Deduplication also prevents a feed repaint loop. */
export function notifyThreadReaders() {
  const signature = JSON.stringify(presentThreads());
  if (signature === readerSignature) return;
  readerSignature = signature;
  readerListeners.forEach((listener) => listener());
}

export function subscribeThreadReaders(listener) {
  readerListeners.add(listener);
  const win = globalThis.window, doc = globalThis.document;
  win?.addEventListener("focus", notifyThreadReaders);
  win?.addEventListener("blur", notifyThreadReaders);
  doc?.addEventListener("visibilitychange", notifyThreadReaders);
  return () => {
    readerListeners.delete(listener);
    if (readerListeners.size) return;
    win?.removeEventListener("focus", notifyThreadReaders);
    win?.removeEventListener("blur", notifyThreadReaders);
    doc?.removeEventListener("visibilitychange", notifyThreadReaders);
  };
}

const matches = (scope, deviceId, entityId, kind) => {
  const address = addressOf(scope.address);
  return address?.deviceId === deviceId && address?.entityId === entityId
    && address?.kind === kind && scope.active();
};

export const visibleTreePaths = (deviceId, entityId) => [...new Set(
  [...trees].filter((scope) => matches(scope, deviceId, entityId, "tree"))
    .flatMap((scope) => scope.value()),
)];

export const visibleWorkingDiff = (deviceId, entityId) =>
  [...diffs].some((scope) => matches(scope, deviceId, entityId, "diff"));

/** A mounted pane can stay connected while its root/tab is folded away. */
export function syncPaneShown(element) {
  if (!element?.isConnected) return false;
  if (element.ownerDocument.hidden) return false;
  const view = element.ownerDocument.defaultView;
  for (let current = element; current; current = current.parentElement) {
    if (current.hidden) return false;
    const style = view.getComputedStyle(current);
    if (style.display === "none" || style.visibility === "hidden") return false;
  }
  return true;
}
