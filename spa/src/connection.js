// Session lifecycle: one session per online device — open, adopt, degrade
// offline, resume — and which of them the app calls home.
//
// Every device answers for itself. The relay pushes device_offline the instant
// one of our bridges drops; that session's own close event covers relay/network
// loss. Degrade that device quietly: its rows stay in the rail (greyed), its
// calls pause, its scope and drafts survive, and it resumes silently the moment
// it is back. The account-wide banner is for the state where nothing at all is
// reachable, which is the only one the user can do anything about.

import * as transport from "@build/secure-transport";
import { RELAY_URL } from "./config.js";
import { openRelaySession } from "./core/session.js";
import { openPeerLink } from "./core/peerLink.js";
import { isSignaling } from "./core/sessionSwitch.js";
import { fetchGatewayToken, fetchIceServers } from "./api.js";
import { App, rememberSelectedDevice } from "./app.js";
import {
  adoptDeviceSession,
  canAnswer,
  closeQuietly,
  contextFor,
  homeContext,
  liveContexts,
  retireDeviceContext,
  setContextOffline,
} from "./core/deviceContexts.js";
import {
  markDeviceOnline,
  markDeviceOffline,
  pinnedDeviceTransportKey,
} from "./devices.js";
import { followTerminalDevice, terminalDeviceId } from "./terminal/manager.js";
import { flushCaptures } from "./core/composeView.js";
import { dispatchChangeEvent, greetBridge } from "./core/changeEvents.js";
import { deliverFeed, joinFeed } from "./core/taskFeed.js";

/** What every socket to this account's relay needs, whoever is opening it and
 *  whatever they mean to do with it: the endpoint, the crypto, and the device
 *  the relay is to land the session on. */
const relayDial = (deviceId) => ({
  relayUrl: RELAY_URL,
  transport,
  WebSocketImpl: WebSocket,
  fetchToken: fetchGatewayToken,
  getPinnedDeviceKey: pinnedDeviceTransportKey,
  preferDeviceId: deviceId,
});

/**
 * A session for one device, with everything that varies by device closed over
 * that device: what pausing means, what being lost means, and whose surfaces a
 * push wakes.
 *
 * The relay only ever lands a session on `preferDeviceId`, so the device is
 * known before the socket is and nothing here has to ask who answered.
 */
export function openDeviceSession(deviceId, { waitForDevice = false } = {}) {
  return openRelaySession({
    ...relayDial(deviceId),
    waitForDevice,
    // One device's offline state pauses one device's calls.
    isPaused: () => contextFor(deviceId)?.offline === true,
    onDeviceKey: markDeviceOnline,
    onDeviceOffline: markDeviceOffline,
    onLost: () => goOffline(deviceId),
    // The bridge saying something moved. A frame nobody asked for reaches the
    // surfaces showing that device's state, which is what lets its polls stand
    // down — except the signaling pushes, which belong to the upgrade
    // negotiating them and describe nothing the surfaces show.
    onPush: (payload) => {
      if (!isSignaling(payload.type)) dispatchChangeEvent(payload, deviceId);
    },
  });
}

/** A settings page owns its connection: nothing about it reaches the registry,
 * so no device the app is already holding is disturbed, and no other device
 * being offline pauses its RPCs. */
export async function openDeviceSettingsSession(deviceId, { onLost } = {}) {
  const session = await openRelaySession({ ...relayDial(deviceId), waitForDevice: false, onLost });
  if (session.deviceId !== deviceId) {
    session.close();
    throw new Error("Could not connect to the requested device.");
  }
  return session;
}

// ---- the peer path (spec §SPA carrier and migration policy) ------------------

/**
 * Upgrade a live session onto a direct peer path, in the background.
 *
 * The user is already working over the relay: this fetches ICE servers, offers
 * over the relay carrier, and migrates both streams once the two channels are
 * open. A failure anywhere logs and leaves the session exactly where it is —
 * the relay is the fallback, not a retry target, so the next attempt is the
 * next relay session for that device and nothing sooner.
 */
async function upgradeToPeer(context) {
  if (!globalThis.RTCPeerConnection) return;
  const session = context.session;
  let link;
  try {
    link = await openPeerLink({
      // Every `rtc.*` call rides the relay for the peer's life — the session's
      // own rule, not this layer's — and waits for it while it reconnects.
      signal: (method, params) => session.call(method, params),
      fetchIceServers,
      onPush: session.onPush,
    });
  } catch (error) {
    console.warn("staying on the relay:", error.message);
    return;
  }
  adoptPeerLink(context, session, link);
}

/** Put this device's streams on the connection that just opened — unless the
 *  session it was opened for is not the one that device is on any more. */
function adoptPeerLink(context, session, link) {
  if (contextFor(session.deviceId)?.session !== session) {
    link.close(); // a newer session for this device overtook the upgrade
    return;
  }
  context.peerLink = link;
  // The two channels are one connection: whichever goes first takes the other,
  // and both streams migrate back to the relay together.
  for (const carrier of [link.app, link.term]) {
    carrier.onClose(() => {
      if (context.peerLink === link) dropPeerLink(context);
    });
  }
  session.peer(link.app);
  followTerminalsIfTheirs(context);
}

/** Idempotent, and the single point where both streams are handed back at once:
 *  nothing may end up on the relay while the other half still rides a peer
 *  connection that is going away. */
function dropPeerLink(context) {
  const link = context?.peerLink;
  if (!link) return;
  context.peerLink = null;
  context.session?.peer(null);
  followTerminalsIfTheirs(context);
  link.close();
}

/** One device's peer link opened or closed. The terminals move only when they
 *  are on that device — another device's channel carries the stream to the
 *  wrong machine, and nothing about the wire theirs rides has changed. */
function followTerminalsIfTheirs(context) {
  if (context.deviceId === terminalDeviceId()) followTerminalDevice();
}

/**
 * Let a device go for good: the account no longer has it.
 *
 * The registry forgets the machine and tells every surface standing over it,
 * but two things it is holding are this layer's. The direct connection it may
 * be riding: retiring through the registry alone would leave an
 * RTCPeerConnection open for the life of the tab, with both streams still
 * pointed down it. And the backoff of a device that never connected at all,
 * which is kept off to the side and would go on asking the relay for a machine
 * the account no longer has.
 *
 * A resume already parked on the relay's `device_key` for this device is not
 * reached: openRelaySession has no abort, so that socket is held until the
 * bridge answers it, and `stillWaiting` closes the session it lands.
 */
export function retireDevice(deviceId) {
  dropPeerLink(contextFor(deviceId));
  forgetUnconnected(deviceId);
  return retireDeviceContext(deviceId);
}

/** Greet a device that is live and unpaused: feature-detect push invalidation,
 *  subscribe that session to it, and read everything it has once. Not awaited by
 *  its callers — a slow greeting must not hold up the app, and a surface mounted
 *  before it lands is re-timed the moment it does. */
export function greetLiveBridge(context) {
  const session = context?.session;
  if (!session) return Promise.resolve(false);
  const repository = context.chatRepository;
  return greetBridge(session.call, {
    deviceId: session.deviceId,
    // The device's context may have been retargeted onto a newer session while
    // this greeting was in flight; that greeting belongs to the session that
    // asked for it, not to the one the device is on now.
    isCurrent: () => contextFor(session.deviceId)?.session === session,
    onGreeting: (greeting) => repository?.configureCapabilities(greeting),
  }).catch(() => {
    /* the session died mid-greeting; the next one greets again */
  });
}

// ---- landing a session, and which device is home -----------------------------

/** Everything a device gets the moment it has a live session: the registry
 *  adopts it (a reconnect keeps that device's scope, drafts and controllers and
 *  only replaces its transport), the feed starts reading it, the bridge is
 *  greeted, and the peer upgrade runs in the background. */
function landSession(session) {
  const previous = contextFor(session.deviceId);
  dropPeerLink(previous); // it was carrying the session this one replaces
  // A device holds one session: a resume and a device that came back online can
  // both land one, and the socket that lost the race is nobody's.
  if (previous?.session !== session) closeQuietly(previous?.session);
  cancelScheduledResume(session.deviceId);
  const context = adoptDeviceSession(session);
  forgetUnconnected(session.deviceId); // it has a context to keep its backoff on now
  // Every later carrier change re-establishes the session on the wire it took:
  // session.hello, and a read of every mounted surface.
  session.onCarrier(() => greetLiveBridge(context));
  // A device the feed is not polling yet — the account's first session, one a
  // late device just opened — gets its own board watcher and reads at once.
  joinFeed(context);
  syncHome(context);
  greetLiveBridge(context);
  upgradeToPeer(context); // in the background: the user is live already
  return context;
}

// The device followHomeContext was last run for. Home itself is derived — the
// account list and the pick say who it is — so this is not another answer to
// that question, only the record of which one the side effects below were last
// carried out for. Signing out forgets it, so the first device of the next
// account is taken in hand however familiar its name.
let followedHomeId = null;

/** Forget which device home was last followed for (sign-out, teardown). */
export function forgetHomeFollow() {
  followedHomeId = null;
}

/** Take the home device in hand: whose link the terminals ride, whose slice the
 *  surfaces about "here" read, and who is offered the captures nobody could
 *  send. The device picker is not among them — it is a filter over the account
 *  list and says nothing about where creation goes. Home is read off the
 *  account list and the pick, so nothing here writes who it is; everything a
 *  home move touches happens here, once. */
function followHomeContext(context) {
  followedHomeId = context.deviceId;
  // The terminal socket reads the device it wants only as it connects, and a
  // healthy one never reconnects on its own: home moving is one of the two
  // things that makes it drop and re-point (a route change is the other).
  followTerminalDevice();
  // Every surface about "here" — the composer's destinations, the toolbar, the
  // capture decision page, the agent rail — keeps the home device's slice of
  // the snapshot it was last handed. Home moving is news about all of them and
  // about no bridge, so it is told from what the devices have already said.
  deliverFeed();
  if (!context.session || context.offline) return;
  // The gate's first session, a reconnect, a new home device: this is where
  // captures taken with no device to send them to are handed over.
  flushCaptures().catch(() => {
    /* still unreachable: the queue keeps them for the next session */
  });
}

/**
 * Send new projects and captures to this machine from now on.
 *
 * The account's one control for home (Settings → Creation device), and its only
 * writer: the pick is remembered, and whoever home is now is taken in hand.
 * Nothing is opened and nothing is closed — every paired device that can answer
 * is already live, and this says only where creation goes.
 */
export function chooseCreationDevice(deviceId) {
  rememberSelectedDevice(deviceId);
  syncHome();
}

/**
 * Catch the side effects up with whoever home is now.
 *
 * Nobody holds home: the account's device list and the user's pick say who it
 * is, and this is asked whenever one of those, or the home device itself, has
 * changed. `landed` is the context whose own state just changed — a device that
 * just landed or just went — and is taken in hand again even when it was
 * already home, because what the surfaces read off it is not what it was.
 */
export function syncHome(landed = null) {
  const followed = followedHomeId;
  // Nothing at all is online, so the account names no home: the side effects
  // stay on the device they were already following and run for it again, which
  // is how the picker and the composer come to say it has gone offline. At boot
  // nothing has been followed yet, which is how the device the pick names keeps
  // home while it is still handshaking.
  const home = homeContext() || contextFor(followed);
  if (home && (home.deviceId !== followed || home === landed)) followHomeContext(home);
}

// ---- opening every device ----------------------------------------------------

/** Connect one device and land it. A device that will not answer is marked
 *  offline — its rows stay, greyed — and kept after until it does. */
async function connectDevice(deviceId) {
  try {
    return landSession(await openDeviceSession(deviceId));
  } catch (error) {
    setContextOffline(deviceId);
    scheduleResume(deviceId);
    throw error;
  }
}

/**
 * Open every online device that has no live session, all at once.
 *
 * `first` is for a caller that needs A device — the gate, so the app starts on
 * whichever machine answers rather than on the slowest one; `settled` is every
 * context that came up. Nothing waits on the slowest device.
 */
export function openDeviceSessions() {
  const attempts = App.devices.filter(wantsSession).map((device) => connectDevice(device.id));
  return { first: handled(firstContext(attempts)), settled: handled(everyContext(attempts)) };
}

/** A device for this call to open: online by the account list, with nothing
 *  already working on it. Already answering is the registry's own question
 *  (canAnswer), asked here the way every surface asks it. A resume is working
 *  on it too — it is parked on the relay's `device_key` for exactly that bridge
 *  and lands the moment it is back, so the push that says so must not start a
 *  second handshake. */
function wantsSession(device) {
  const context = contextFor(device.id);
  return device.status === "online" && !canAnswer(context) && !context?.reconnect.resuming;
}

/** A caller usually wants one of the two promises. Handling the other here
 *  keeps a refused device from reading as an unhandled rejection, and leaves
 *  what the caller awaits exactly as it was. */
function handled(promise) {
  promise.catch(() => {});
  return promise;
}

function firstContext(attempts) {
  if (attempts.length) return Promise.any(attempts);
  // Nothing to open: either every online device is already live — and whoever
  // asked can use one of those — or there is no device to answer at all.
  const [live] = liveContexts();
  return live ? Promise.resolve(live) : Promise.reject(new Error("No device answered."));
}

async function everyContext(attempts) {
  const results = await Promise.allSettled(attempts);
  return results.filter((result) => result.status === "fulfilled").map((result) => result.value);
}

// ---- offline and resume, per device ------------------------------------------

/** One device stopped answering. Its context stays registered: its rows stay in
 *  the rail, its scope and drafts survive, and only its own calls pause. */
export function goOffline(deviceId) {
  const context = contextFor(deviceId);
  if (!context || context.offline) return;
  setContextOffline(deviceId);
  dropPeerLink(context);
  closeQuietly(context.session);
  // Home may have moved off it — and if it has not, the surfaces that follow
  // home still have to say the device they are about is offline.
  syncHome(context);
  resume(deviceId);
}

/** Keep asking for one device until it answers. `waitForDevice` blocks on the
 *  relay's `device_key` for exactly this device: the fresh authenticated socket
 *  hears it the moment that bridge is back. */
export async function resume(deviceId) {
  const reconnect = reconnectFor(deviceId);
  if (reconnect.resuming) return;
  reconnect.resuming = true;
  clearTimeout(reconnect.timer);
  try {
    await claimResumedSession(deviceId, reconnect);
  } catch {
    scheduleResume(deviceId, reconnect); // the relay is unreachable too — back off
  } finally {
    reconnect.resuming = false;
  }
}

async function claimResumedSession(deviceId, reconnect) {
  const waiting = contextFor(deviceId);
  const session = await openDeviceSession(deviceId, { waitForDevice: true });
  if (!stillWaiting(deviceId, waiting)) {
    closeQuietly(session);
    return;
  }
  reconnect.delay = 0;
  landSession(session);
}

/** Whether the session that just landed is still wanted: a new home device or
 *  another resume can restore a device while this one waits, and a device that
 *  was retired is not coming back at all. */
function stillWaiting(deviceId, waiting) {
  const context = contextFor(deviceId);
  if (waiting) return context === waiting && context.offline;
  return !context; // a device that had never connected still has not
}

/** A resume still counting down for this device is waiting for exactly what
 *  just landed. Left armed it fires at a device that is live again: another
 *  handshake, another greeting, and a session for the bin. */
function cancelScheduledResume(deviceId) {
  const reconnect = contextFor(deviceId)?.reconnect || unconnected.get(deviceId);
  if (reconnect) clearTimeout(reconnect.timer);
}

function scheduleResume(deviceId, reconnect = reconnectFor(deviceId)) {
  reconnect.delay = Math.min((reconnect.delay || 1000) * 2, 15000);
  clearTimeout(reconnect.timer);
  reconnect.timer = setTimeout(() => resume(deviceId), reconnect.delay);
}

// A device whose very first connect failed has no context to keep its backoff
// on; it waits here until it has one. Every other device's is on its context,
// so retiring a device retires its reconnect with it.
const unconnected = new Map();

function reconnectFor(deviceId) {
  const context = contextFor(deviceId);
  if (context) {
    forgetUnconnected(deviceId);
    return context.reconnect;
  }
  if (!unconnected.has(deviceId)) unconnected.set(deviceId, { timer: null, delay: 0, resuming: false });
  return unconnected.get(deviceId);
}

/** Let go of a backoff kept here: the device has a context to keep its own on
 *  now, or it is gone. The timer goes with it — left armed it asks for a
 *  machine nobody is waiting for. */
function forgetUnconnected(deviceId) {
  const reconnect = unconnected.get(deviceId);
  if (!reconnect) return;
  clearTimeout(reconnect.timer);
  unconnected.delete(deviceId);
}
