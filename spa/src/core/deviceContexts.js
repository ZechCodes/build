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
import { creationDeviceId, homeDeviceId } from "./devicePolicy.js";
import { createChatRepository } from "./chatRepository.js";
import { deviceView, feedHoldsDevice } from "./feedMerge.js";
import { createModelCatalog } from "./modelCatalog.js";
import { disarmChangeEvents } from "./changeEvents.js";
import { deviceAwayMark } from "./deviceAway.js";
import { dropFeedDevice } from "./taskFeed.js";
import { createDeviceLifecycles } from "./deviceLifecycle.js";

const contexts = new Map(); // deviceId → context, in the order they were adopted
const deviceLifecycles = createDeviceLifecycles();
const stateListeners = new Set(); // told when a device's ability to answer changes
let registryEra = 0;
const deviceEras = new Map();

export const deviceContextEra = () => registryEra;
export const deviceContextIdentity = (deviceId) => `${registryEra}:${deviceEras.get(deviceId) || 0}`;

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

/**
 * Something about HOW these machines are reachable changed — not whether.
 *
 * A peer connection that re-nominates onto a direct pair (issue #31) is the same
 * machine, answering as before, so nothing here has become available or
 * unavailable. What has changed is the one thing about the connection the reader
 * cannot otherwise see, and it is shown in the same places the availability is
 * (the ring, and the row behind it), so it is announced down the same channel
 * rather than through a second one nobody else subscribes to.
 */
export const announceDeviceTransport = () => announceDeviceState();

/**
 * A caller to one machine that refuses when that machine cannot answer.
 *
 * Every surface asks canAnswer before it stands a frame up, but a poll already
 * running, a watcher already subscribed and the cache syncer's background tier
 * ask nobody — they hold a caller from mount and call it. A greeting that
 * settles unsupported under them (a bridge updated past this tab re-greets on
 * its next session) would otherwise leave them asking for answers in a shape
 * this tab cannot read, and painting whatever came back. So the refusal lives
 * at the caller, where every one of them passes, and says which side is behind
 * in the same words the strip over that surface says.
 *
 * `transport` is which session to ask: the device's current one for the caller
 * the surfaces hold, and the one that adopted it for the conversations, which
 * finish on the transport that accepted them. Everything else — `(method,
 * params, timeoutMs)` — is passed straight through, so an argument the caller
 * left out stays left out and the session's own defaults apply.
 */
const asking = (context, transport) => (...asked) =>
  (canAnswer(context) ? transport()(...asked) : Promise.reject(new Error(deviceAwayMark(context))));

function createDeviceContext(deviceId) {
  const lifecycle = deviceLifecycles.forDevice(deviceId);
  const context = {
    deviceId,
    rpc: null, // asking this machine; stood up below, over the context itself
    cacheScope: scopeFor(deviceId),
    chatRepository: null,
    adapter: null, // the API adapter this bridge's greeting installed, or null
    apiVersion: null, // the `api_version` it greeted with
    unsupported: null, // "app" | "bridge" when no adapter here speaks to it
    greeted: null, // this session's greeting, once one is in flight (connection.js)
    /** Still the registry's context for this device, and still able to address
     *  the cache: what a late answer must ask before it writes anything. */
    active: () => contexts.get(deviceId) === context && Boolean(context.cacheScope?.active()),
  };
  for (const field of ["session", "call", "peerLink", "offline", "offlineSince", "blocked"]) {
    Object.defineProperty(context, field, {
      enumerable: true,
      get: () => lifecycle.snapshot()[field],
    });
  }
  /** The one spelling of "ask this machine", and the only caller anything
   *  outside this module holds. A reconnect replaces the transport under a
   *  surface that is still mounted: a caller captured at mount would go on
   *  asking a session that is closed, and every call it made would be refused
   *  for want of a carrier on a surface still claiming to be live. This reads
   *  whichever session the device is on when the call is made; a call already
   *  in flight settles on the session that accepted it. */
  context.rpc = asking(context, () => context.call);
  // What this bridge offers to start work with, held here rather than on the
  // app: the machine is what the answer is about (core/modelCatalog.js).
  Object.assign(context, createModelCatalog(context));
  contexts.set(deviceId, context);
  return context;
}

export function contextFor(deviceId) {
  return (deviceId && contexts.get(deviceId)) || null;
}

/**
 * This machine's context, created empty if it has never had one.
 *
 * A device gets a context by answering — but a machine whose connect sequence
 * failed has answered nothing and still has something to say about itself: rule
 * 3 blocks it, by name, with a reason, and its rows grey like any other away
 * machine's. So it is registered with no session: it can answer nothing
 * (`canAnswer` is false), it is in `knownContexts` and never in `liveContexts`,
 * and the session it eventually lands retargets this same context.
 */
export function knownDeviceContext(deviceId) {
  return contextFor(deviceId) || createDeviceContext(deviceId);
}

/**
 * What this device's greeting settled: the adapter its session installed, the
 * API version it reported, and which side is behind when no adapter here speaks
 * to it. Written in this one place, so every surface reads the same three
 * fields whichever machine it is about.
 */
export function adoptBridgeSelection(context, selection, adapter) {
  if (!context) return null;
  context.adapter = adapter || null;
  context.apiVersion = selection?.version || null;
  context.unsupported = selection?.unsupported || null;
  releaseGreeting(context); // this bridge has said what it speaks
  announceDeviceState(); // an unsupported bridge is a machine that cannot answer
  return context;
}

/** A device whose greeting no longer stands: the next one settles it again. */
function forgetBridgeSelection(context) {
  context.adapter = null;
  context.apiVersion = null;
  context.unsupported = null;
  releaseGreeting(context); // whoever was waiting on the last one waits on the next
  armGreeting(context);
}

// What releases each device's armed greeting, kept beside the contexts rather
// than on them: a promise's own settle is not something a surface should be
// able to reach for.
const greetingReleases = new Map(); // context → { promise, release }

/**
 * Arm this device's greeting: the promise a reader that must not ask before the
 * bridge has answered waits on.
 *
 * A session is adopted before it is greeted, and the machine answering a
 * reconnect may not be the one that answered last — a bridge is restarted,
 * updated, or replaced — so what the last greeting settled says nothing about
 * this one. The feed is the reader (core/taskFeed.js): it goes to the session
 * directly, so it is the one read that would otherwise be sent before this
 * bridge had said which API major it speaks.
 */
function armGreeting(context) {
  let release;
  const promise = new Promise((settle) => { release = settle; });
  const token = { promise, release };
  greetingReleases.set(context, token);
  context.greeted = promise;
}

export const greetingToken = (context) => greetingReleases.get(context) || null;

/** This device's greeting has settled, however it settled: an adapter was
 *  selected, a side was named behind, or the session died with nothing said.
 *  connection.js releases that last one, so a lost greeting never leaves a
 *  device unread. */
export function releaseGreeting(context, token) {
  if (!context) return;
  const current = greetingReleases.get(context);
  if (arguments.length > 1 && current !== token) return;
  current?.release();
  greetingReleases.delete(context);
}

/**
 * Whether this machine can be asked anything right now.
 *
 * Having a context is not the same as being able to reach the machine: one that
 * answered once keeps its context through an outage — the drafts and cached
 * reads held against it outlive the connection — and a link can name a machine
 * this client has never opened at all, which has none. A bridge speaking an API
 * major nothing here claims is the third way: the socket is up, and every answer
 * off it would be a guess at a shape. Every surface that would stand a frame up
 * over a device asks here, so the question is asked one way.
 */
export const canAnswer = (context) =>
  Boolean(context && context.call && !context.offline && !context.unsupported);

/**
 * A context stood up over a machine's records before that machine has ever
 * answered (core/surfaceContext.js): no session, and no mark saying why not —
 * every stand-down stamps `offlineSince`. Nothing has been tried and failed,
 * so whatever speaks for the machine's reachability treats it as a machine
 * this client has not opened yet.
 */
export const awaitingFirstAnswer = (context) => Boolean(context) && !context.session && !context.offlineSince;

/** Every registered device, offline ones included, in App.devices order —
 *  devices the list has not caught up with yet keep their adoption order last. */
export function knownContexts() {
  const order = new Map(App.devices.map((device, index) => [device.id, index]));
  const rank = (context) => order.get(context.deviceId) ?? order.size;
  return [...contexts.values()].sort((first, second) => rank(first) - rank(second));
}

/** The contexts that can answer right now, in the account's own order. Asked
 *  through canAnswer, so "live" here and "can this machine be asked anything"
 *  everywhere else are one question with one answer. */
export function liveContexts() {
  return knownContexts().filter(canAnswer);
}

/** Create this device's context or retarget the one it already has. A
 *  reconnect keeps the scope and the repository and only takes the new
 *  transport; a device seen for the first time gets both. */
export function adoptDeviceSession(session) {
  return adoptDeviceConnection(session).context;
}

/** Atomically hand an established app session and its peer resources to the
 * device owner. The returned token is the only authority callbacks from this
 * connection lifetime may use. */
export function adoptDeviceConnection(session, peerLink = null, onDetached = () => {}) {
  const context = contextFor(session.deviceId) || createDeviceContext(session.deviceId);
  const lifetime = deviceLifecycles.forDevice(session.deviceId).adopt({ session, peerLink, onDetached });
  if (!lifetime.current() || contexts.get(session.deviceId) !== context) {
    return { context, lifetime };
  }
  // A reconnect re-greets, and the bridge answering it may not be the version
  // that answered last time: what the last greeting settled is not this one's.
  forgetBridgeSelection(context);
  // The repository is bound to the session that adopted it, not to whichever
  // session the device is on: a post captures its caller when it is made and
  // must finish where it was accepted, or a reconnect landing mid-flight sends
  // it twice. It still refuses when the machine cannot answer.
  bindRepository(context, asking(context, () => session.call));
  announceDeviceState(); // this device can answer again
  return { context, lifetime };
}

export const existingDeviceLifecycle = (deviceId) => deviceLifecycles.existing(deviceId);
export const deviceSecurityStopText = () => deviceLifecycles.securityStopText();
export const clearDeviceSecurityStops = () => deviceLifecycles.clearSecurityStops();

function commandDeviceLifecycle(deviceId, command) {
  const context = knownDeviceContext(deviceId);
  const changed = command(deviceLifecycles.forDevice(deviceId), context);
  if (changed) announceDeviceState();
  return { context, changed };
}

function commandCapturedLifetime(deviceId, lifetime, command) {
  const context = contexts.get(deviceId);
  const owner = deviceLifecycles.existing(deviceId);
  if (!context || !owner) return { context: null, changed: false };
  const changed = command(owner, lifetime);
  if (changed) announceDeviceState();
  return { context, changed };
}

export const loseDeviceConnection = (deviceId, lifetime) =>
  commandCapturedLifetime(deviceId, lifetime, (owner, captured) => owner.lose(captured));

export const blockDeviceConnection = (deviceId, lifetime, reason) =>
  commandCapturedLifetime(deviceId, lifetime, (owner, captured) => owner.block(captured, reason));

export const blockCurrentDevice = (deviceId, reason) =>
  commandDeviceLifecycle(deviceId, (owner) => owner.blockCurrent(reason));

export const markDevicePresenceAway = (deviceId) =>
  commandDeviceLifecycle(deviceId, (owner) => owner.presenceAway());

export const refuseDeviceConnection = (deviceId, message) =>
  commandDeviceLifecycle(deviceId, (owner) => owner.refuse(message));

export const retryDeviceConnection = (deviceId) =>
  commandDeviceLifecycle(deviceId, (owner) => owner.retry());

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
 *  addressing the cache, its session is closed, and every surface standing over
 *  it is told it can answer nothing now. Another device's context is untouched.
 *
 *  The transport is all this reaches: a device riding a direct connection is
 *  retired through connection.js, which hands both streams back first. */
export function retireDeviceContext(deviceId) {
  const context = contexts.get(deviceId);
  if (!context) return null;
  contexts.delete(deviceId);
  deviceEras.set(deviceId, (deviceEras.get(deviceId) || 0) + 1);
  context.chatRepository?.dispose();
  context.disposeModelCatalog?.();
  dropFeedDevice(deviceId);
  disarmChangeEvents(deviceId);
  releaseScope(deviceId);
  deviceLifecycles.retire(deviceId);
  releaseGreeting(context); // nothing will greet it now
  announceDeviceState(); // this device can answer nothing, ever again
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
    deviceLifecycles.forDevice(deviceId).setAvailability(mark);
    announceDeviceState();
  }
  return context || null;
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
  return contextFor(homeDeviceId(App.devices, App.selectedDeviceId, stillWorthAsking));
}

/**
 * Whether home is still this machine's to claim.
 *
 * It is, unless this client has given up on it: a context wearing a reason it
 * cannot answer — blocked, away, speaking an API nothing here reads. A machine
 * with no context at all has not failed, it has not answered YET (a dial in
 * flight, a boot that has not reached it), and taking home off it would hand
 * every account's home to whichever bridge shook hands quickest. Nor has one
 * whose context a page stood up over its records before it answered
 * (core/surfaceContext.js): that context wears no mark.
 */
const stillWorthAsking = (deviceId) => {
  const context = contexts.get(deviceId);
  return !context || canAnswer(context) || awaitingFirstAnswer(context);
};

/**
 * The machine creation goes to, named even while nothing can take it
 * (core/devicePolicy.js).
 *
 * Asked here rather than of the policy directly, so the machine a surface says
 * a capture is for is the machine `homeContext` would hand it to. A surface
 * naming one and a send addressing another is how a composer promises a
 * workspace on the laptop and refuses on the desktop.
 */
export const creationDevice = () =>
  creationDeviceId(App.devices, App.selectedDeviceId, stillWorthAsking);

/** The context a route is about: work surfaces are about the machine their link
 *  names. A route that names no device, or one this client has never opened, is
 *  about nobody. */
export function routeContext(route) {
  return contextFor(route?.deviceId);
}

/**
 * One device's slice of a merged feed snapshot: its rows, its projects, the
 * live run behind each row, and the checkouts the board does not list.
 *
 * A surface about where you are — the toolbar, the capture decision page, a
 * branch — is about one machine, and so is what a row's verb offers: a reroute
 * names a project by the bare id the daemon holding it minted, and every daemon
 * mints a `proj-1`. Naming no device means the home device, which is where
 * creation goes when nothing else says.
 *
 * Named collections rather than the whole view: a slice is what a surface is
 * handed, and a field nobody here names is a field no surface may quietly come
 * to depend on. `runs` carries what a branch row does not — the goal, the
 * state, the base a diff is measured against — and
 * `externalWorktrees` carries the checkouts the inbox leaves out, which a link
 * to one still has to open.
 */
export function deviceFeedView(snapshot, deviceId = null) {
  const view = deviceView(snapshot, deviceId || homeContext()?.deviceId);
  return {
    items: view.items || [],
    projects: view.projects || [],
    runs: view.runs || [],
    externalWorktrees: view.externalWorktrees || [],
    workspaces: view.workspaces || [],
  };
}

/** Whether that snapshot carries this machine's own records at all.
 *
 *  False while the merge names other machines and not this one — a deep link
 *  that landed before this machine's first pass wrote anything. `deviceFeedView`
 *  answers the empty view for that machine and for one that listed nothing
 *  alike, and a surface that has to tell "not loaded" from "not there" asks
 *  this first. Naming no device means the home device, as everywhere else. */
export function deviceFeedHeld(snapshot, deviceId = null) {
  return feedHoldsDevice(snapshot, deviceId || homeContext()?.deviceId);
}

export function resetDeviceContexts() {
  registryEra += 1;
  for (const deviceId of [...contexts.keys()]) retireDeviceContext(deviceId);
  contexts.clear();
  deviceLifecycles.clear();
  deviceEras.clear();
}
