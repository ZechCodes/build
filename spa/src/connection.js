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
import { $ } from "./dom.js";
import { RELAY_URL } from "./config.js";
import { openRelaySession } from "./core/session.js";
import { openPeerLink } from "./core/peerLink.js";
import { isSignaling } from "./core/sessionSwitch.js";
import { onlineStickyDeviceId } from "./core/devicePolicy.js";
import { fetchGatewayToken, fetchIceServers } from "./api.js";
import { App, pointAliasesAt, render, rememberSelectedDevice } from "./app.js";
import {
  adoptDeviceSession,
  contextFor,
  homeContext,
  knownContexts,
  liveContexts,
  setContextOffline,
} from "./core/deviceContexts.js";
import {
  deviceName,
  markDeviceOnline,
  markDeviceOffline,
  paintDevicePicker,
  pinnedDeviceTransportKey,
} from "./devices.js";
import { retargetTerminals, terminalsRideOn } from "./terminal/manager.js";
import { flushCaptures } from "./core/composeView.js";
import { dispatchChangeEvent, greetBridge } from "./core/changeEvents.js";
import { joinFeed } from "./core/taskFeed.js";
import { allDevicesOfflineText, offlineBannerText } from "./core/text.js";

const CONNECTED = '<span class="dot"></span>connected';
const RECONNECTING = '<span class="dot" style="background:var(--amber)"></span>reconnecting…';

/// Connection status has no chip of its own any more — the status line under the
/// rail is the device picker and nothing else. Offline still speaks up loudly
/// through the banner (#offbar), which is the state that actually needs saying.
/// Kept as a no-op-when-absent writer so every caller stays unchanged.
export function setConn(html) {
  const el = $("#conn");
  if (el) el.innerHTML = html;
}

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
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    getPinnedDeviceKey: pinnedDeviceTransportKey,
    preferDeviceId: deviceId,
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

/** A settings page owns its connection: it never changes the active workspace,
 * and the active device's offline state must not pause another device's RPCs. */
export async function openDeviceSettingsSession(deviceId, { onLost } = {}) {
  const session = await openRelaySession({
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    getPinnedDeviceKey: pinnedDeviceTransportKey,
    preferDeviceId: deviceId,
    waitForDevice: false,
    onLost,
  });
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
  handTerminalsToHome(context);
}

/** Idempotent, and the single point where both streams are handed back at once:
 *  nothing may end up on the relay while the other half still rides a peer
 *  connection that is going away. */
function dropPeerLink(context) {
  const link = context?.peerLink;
  if (!link) return;
  context.peerLink = null;
  context.session?.peer(null);
  handTerminalsToHome(context);
  link.close();
}

/** The terminal socket rides the home device's peer channel and nobody else's —
 *  another device's channel carries the stream to the wrong machine. Asked
 *  whenever either half of that pair moves: the home device's link opening or
 *  closing, and home moving to another device. */
function handTerminalsToHome(context = null) {
  if (context && homeContext() !== context) return;
  terminalsRideOn(homeContext()?.peerLink?.term || null);
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
  dropPeerLink(contextFor(session.deviceId)); // it was carrying the session this replaces
  const context = adoptDeviceSession(session);
  // Every later carrier change re-establishes the session on the wire it took:
  // session.hello, and a read of every mounted surface.
  session.onCarrier(() => greetLiveBridge(context));
  // A device the feed is not polling yet — the account's first session, one a
  // late device just opened — gets its own board watcher and reads at once.
  joinFeed(context);
  settleHome(context);
  paintOfflineBanner();
  greetLiveBridge(context);
  upgradeToPeer(context); // in the background: the user is live already
  return context;
}

/** Where creation goes, which context the App.* aliases follow, and whose link
 *  carries the terminals. Re-asked whenever that context's offline state
 *  changes: the aliases are plain fields, and App.offline must never lie to the
 *  composer or to a frozen view. */
function followHomeContext(context) {
  pointAliasesAt(context);
  handTerminalsToHome();
  paintDevicePicker();
  if (!context?.session || context.offline) return;
  // The gate's first session, a reconnect, a new home device: this is where
  // captures taken with no device to send them to are handed over.
  flushCaptures().catch(() => {
    /* still unreachable: the queue keeps them for the next session */
  });
}

/** Home is chosen once and never moves on its own: the sticky device claims it
 *  when it answers, and a device that lands later takes nothing from whoever
 *  holds it. A landing by the home device itself is that device coming back. */
function settleHome(context) {
  const home = homeContext();
  if (home) {
    if (home === context) followHomeContext(context);
    return;
  }
  if (onlineStickyDeviceId(App.devices, App.selectedDeviceId) === context.deviceId) followHomeContext(context);
}

/** Name a context home when no device holds it yet — what the gate does with
 *  the first device that answers. */
export function claimHomeContext(context) {
  if (!homeContext()) followHomeContext(context);
}

/** Move home to another device: where creation goes, which context the aliases
 *  follow, and which machine the terminals attach to. It closes nothing — every
 *  other device stays live and keeps filling the inbox. */
export async function setHomeDevice(deviceId) {
  rememberSelectedDevice(deviceId);
  if (!hasLiveSession(deviceId)) await connectDevice(deviceId);
  followHomeContext(contextFor(deviceId));
  retargetTerminals(); // the terminal socket follows the home device
  render();
}

// ---- opening every device ----------------------------------------------------

const hasLiveSession = (deviceId) => liveContexts().some((context) => context.deviceId === deviceId);

/** Connect one device and land it; a device that will not answer is marked
 *  offline and kept after. */
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
  const attempts = App.devices
    .filter((device) => device.status === "online" && !hasLiveSession(device.id))
    .map((device) => connectDevice(device.id));
  return { first: handled(firstContext(attempts)), settled: handled(everyContext(attempts)) };
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
  if (homeContext() === context) followHomeContext(context);
  paintOfflineBanner();
  resume(deviceId);
}

function closeQuietly(session) {
  try {
    session?.close?.();
  } catch {
    /* already gone */
  }
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
    unconnected.delete(deviceId);
    return context.reconnect;
  }
  if (!unconnected.has(deviceId)) unconnected.set(deviceId, { timer: null, delay: 0, resuming: false });
  return unconnected.get(deviceId);
}

// ---- the account-wide banner -------------------------------------------------

/** The banner speaks for the account, not for a device: while anything is
 *  reachable the rail's greyed rows say all there is to say about the one that
 *  is not. */
export function paintOfflineBanner() {
  const nothingLive = liveContexts().length === 0;
  document.body.classList.toggle("offline", nothingLive);
  const banner = $("#offbar");
  if (banner) banner.hidden = !nothingLive;
  if (!nothingLive) {
    setConn(CONNECTED);
    return;
  }
  const text = $("#offbar-text");
  if (text) text.textContent = bannerText(knownContexts());
  setConn(RECONNECTING);
}

/** One device is named, with when it went unreachable; several of them (or none
 *  this client ever reached) is a sentence about the account. */
function bannerText(contexts) {
  if (contexts.length !== 1) return allDevicesOfflineText();
  const [context] = contexts;
  return offlineBannerText(deviceName(context.deviceId) || "Your device", context.offlineSince || Date.now());
}
