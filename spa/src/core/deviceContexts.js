// One context per paired device: everything the app needs to work that device,
// in one object, so no surface has to ask "which device is this?" again.
//
// A context is created the first time a session for that device is adopted and
// lives until the device is retired: reconnects only replace its transport, so
// the drafts, controllers and cached reads captured against it survive. The App
// module is read lazily inside these functions — app.js imports this module, so
// reading it at load time would read a half-built module.

import { App } from "../app.js";
import { releaseScope, scopeFor } from "./cacheScope.js";
import { homeDeviceId } from "./devicePolicy.js";
import { createChatRepository } from "./chatRepository.js";
import { deviceView } from "./feedMerge.js";
import { disarmChangeEvents } from "./changeEvents.js";
import { dropFeedDevice } from "./taskFeed.js";

const contexts = new Map(); // deviceId → context, in the order they were adopted
const stateListeners = new Set(); // told when a device's ability to answer changes

/**
 * Hear when some device starts or stops being able to answer.
 *
 * Whether a machine is reachable is not news the feed carries — a row does not
 * change when the machine behind it goes — so a surface that greys what a lost
 * device holds is told here instead of waiting out a poll. Returns unsubscribe.
 */
export function onDeviceStateChanged(fn) {
  stateListeners.add(fn);
  return () => stateListeners.delete(fn);
}

const announceDeviceState = () => stateListeners.forEach((fn) => fn());

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
  return (deviceId && contexts.get(deviceId)) || null;
}

/**
 * Whether this machine can be asked anything right now.
 *
 * Having a context is not the same as being able to reach the machine: one that
 * answered once keeps its context through an outage — the drafts and cached
 * reads held against it outlive the connection — and a link can name a machine
 * this client has never opened at all, which has none. Every surface that would
 * stand a frame up over a device asks here, so the question is asked one way.
 */
export const canAnswer = (context) => Boolean(context && context.call && !context.offline);

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
  announceDeviceState(); // this device can answer again
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

/** Retire a device for good: its controllers and drafts go, its rows leave the
 *  feed, what it pushed stops being anyone's cadence, its scope stops
 *  addressing the cache, and its session is closed. Another device's context is
 *  untouched. */
export function retireDeviceContext(deviceId) {
  const context = contexts.get(deviceId);
  if (!context) return null;
  contexts.delete(deviceId);
  clearTimeout(context.reconnect.timer); // a retired device stops trying to come back
  context.chatRepository?.dispose();
  dropFeedDevice(deviceId);
  disarmChangeEvents(deviceId);
  releaseScope(deviceId);
  closeQuietly(context.session);
  context.session = null;
  context.call = null;
  return context;
}

/** A session nobody wants any more. Closing one that is already gone is not an
 *  error anywhere: the socket may have died before we got to it. */
export function closeQuietly(session) {
  try {
    session?.close?.();
  } catch {
    /* already gone */
  }
}

/** The one writer of a context's offline mark. */
export function setContextOffline(deviceId, mark = {}) {
  const context = contexts.get(deviceId);
  if (context) {
    writeOfflineMark(context, mark);
    announceDeviceState();
  }
  return context || null;
}

// Going offline without a stamp means "as of now"; coming back online has no
// time to keep.
function writeOfflineMark(context, { offline = true, sinceMs = null }) {
  context.offline = Boolean(offline);
  context.offlineSince = context.offline ? sinceMs || Date.now() : null;
}

/**
 * Where creation goes: the home device's context, or null.
 *
 * Home is not a pointer anyone writes — it is what the account list and the
 * user's pick already say (core/devicePolicy.js), so nothing can hold home
 * while the account calls another device the home one. A device the policy
 * names before it has answered has no context yet, and home is nobody's until
 * it does.
 */
export function homeContext() {
  return contextFor(homeDeviceId(App.devices, App.selectedDeviceId));
}

/** The context a route is about: work surfaces are about the machine their link
 *  names. A route that names no device, or one this client has never opened, is
 *  about nobody. */
export function routeContext(route) {
  return contextFor(route?.deviceId);
}

/**
 * One device's slice of a merged feed snapshot: its rows and its projects.
 *
 * A surface about where you are — the toolbar, the capture decision page, a
 * branch — is about one machine, and so is what a row's verb offers: a reroute
 * names a project by the bare id the daemon holding it minted, and every daemon
 * mints a `proj-1`. Naming no device means the home device, which is where
 * creation goes when nothing else says.
 */
export function deviceFeedView(snapshot, deviceId = null) {
  const view = deviceView(snapshot, deviceId || homeContext()?.deviceId);
  return { items: view.items || [], projects: view.projects || [] };
}

export function resetDeviceContexts() {
  for (const deviceId of [...contexts.keys()]) retireDeviceContext(deviceId);
  contexts.clear();
}
