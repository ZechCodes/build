// One context per paired device: everything the app needs to work that device,
// in one object, so no surface has to ask "which device is this?" again.
//
// A context is created the first time a session for that device is adopted and
// lives until the device is retired: reconnects only replace its transport, so
// the drafts, controllers and cached reads captured against it survive. The App
// module is read lazily inside these functions — app.js imports this module, so
// reading it at load time would read a half-built module.

import { App } from "../app.js";
import { adoptCacheScope, releaseScope, scopeFor } from "./cacheScope.js";
import { createChatRepository } from "./chatRepository.js";

const contexts = new Map(); // deviceId → context, in the order they were adopted
let homeDevice = null; // the device the App.* aliases were last pointed at

function createDeviceContext(deviceId) {
  const context = {
    deviceId,
    session: null, // { deviceId, call, onPush, peer, onCarrier, close } or null while offline
    call: null, // session.call, retargeted on every re-adoption
    cacheScope: scopeFor(deviceId),
    chatRepository: null,
    offline: false, // written only by setContextOffline (connection.js owns the policy)
    offlineSince: null,
    peerLink: null,
    reconnect: { timer: null, delay: 0, resuming: false },
    /** Still the registry's context for this device, and still able to address
     *  the cache: what a late answer must ask before it writes anything. */
    active: () => contexts.get(deviceId) === context && Boolean(context.cacheScope?.active()),
  };
  contexts.set(deviceId, context);
  return context;
}

export function contextFor(deviceId) {
  return contexts.get(deviceId) || null;
}

/** Every registered device, offline ones included, in App.devices order —
 *  devices the list has not caught up with yet keep their adoption order last. */
export function knownContexts() {
  const order = new Map(App.devices.map((device, index) => [device.id, index]));
  const rank = (context) => order.get(context.deviceId) ?? order.size;
  return [...contexts.values()].sort((first, second) => rank(first) - rank(second));
}

/** The contexts that can answer right now: a session and no offline mark. */
export function liveContexts() {
  return knownContexts().filter((context) => context.session && !context.offline);
}

/** Create this device's context or retarget the one it already has. A
 *  reconnect keeps the scope and the repository and only takes the new
 *  transport; a device seen for the first time gets both. */
export function adoptDeviceSession(session) {
  const context = contextFor(session.deviceId) || createDeviceContext(session.deviceId);
  context.session = session;
  context.call = session.call;
  context.offline = false;
  context.offlineSince = null;
  bindRepository(context, session.call);
  return context;
}

function bindRepository(context, call) {
  if (context.chatRepository) {
    context.chatRepository.retarget(call);
    return;
  }
  // The reader's position is the person's, not the device's: every repository
  // is given the one App.viewingContext.
  context.chatRepository = createChatRepository({
    scope: context.cacheScope,
    call,
    viewingContext: App.viewingContext,
  });
}

/** Retire a device for good: its controllers and drafts go, its scope stops
 *  addressing the cache, and its session is closed. Another device's context is
 *  untouched. */
export function retireDeviceContext(deviceId) {
  const context = contexts.get(deviceId);
  if (!context) return null;
  contexts.delete(deviceId);
  if (homeDevice === deviceId) homeDevice = null;
  context.chatRepository?.dispose();
  releaseScope(deviceId);
  closeQuietly(context.session);
  context.session = null;
  context.call = null;
  return context;
}

function closeQuietly(session) {
  try {
    session?.close?.();
  } catch {
    /* already gone */
  }
}

/** The one writer of a context's offline mark. */
export function setContextOffline(deviceId, { offline = true, sinceMs = null } = {}) {
  const context = contexts.get(deviceId);
  if (!context) return null;
  context.offline = Boolean(offline);
  context.offlineSince = context.offline ? sinceMs || Date.now() : null;
  return context;
}

/** Where creation goes and what the App.* aliases point at. Stage 1's meaning
 *  is literal: the context pointAliasesAt was last handed. Stage 2 redefines it
 *  over homeDeviceId(App.devices, App.selectedDeviceId). */
export function homeContext() {
  return homeDevice ? contextFor(homeDevice) : null;
}

/** Called by pointAliasesAt (app.js) — pointing the aliases at a context and
 *  calling it home are one statement, made in one place. That includes
 *  cacheScope's own home alias, which surfaces still read as
 *  currentCacheScope() while they mount. */
export function setHomeContext(context) {
  homeDevice = context?.deviceId || null;
  adoptCacheScope(homeDevice);
  return context || null;
}

export function resetDeviceContexts() {
  for (const deviceId of [...contexts.keys()]) retireDeviceContext(deviceId);
  contexts.clear();
  homeDevice = null;
}
