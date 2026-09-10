// Session lifecycle: open, adopt, degrade offline, resume, switch devices.
//
// The relay pushes device_offline the instant our bridge drops; the socket's
// own close event covers relay/network loss. Degrade quietly: banner + amber
// dot, freeze the current view (the polls fail fast and skip re-rendering, so
// in-progress typing/reading is untouched), pause bridge-backed actions — then
// resume silently the moment the device is back.

import * as transport from "@build/secure-transport";
import { $ } from "./dom.js";
import { RELAY_URL } from "./config.js";
import { openRelaySession } from "./core/session.js";
import { openPeerLink } from "./core/peerLink.js";
import { isSignaling } from "./core/sessionSwitch.js";
import { onlineStickyDeviceId } from "./core/devicePolicy.js";
import { fetchGatewayToken, fetchIceServers } from "./api.js";
import { App, adoptApplicationScope, render, rememberSelectedDevice } from "./app.js";
import {
  deviceName,
  markDeviceOnline,
  markDeviceOffline,
  paintDevicePicker,
  pinnedDeviceTransportKey,
  refreshDevices,
} from "./devices.js";
import { retargetTerminals, terminalsRideOn } from "./terminal/manager.js";
import { flushCaptures } from "./core/composeView.js";
import { dispatchChangeEvent, greetBridge } from "./core/changeEvents.js";
import { resetFeedScope } from "./core/taskFeed.js";
import { offlineBannerText } from "./core/text.js";

/// Connection status has no chip of its own any more — the status line under the
/// rail is the device picker and nothing else. Offline still speaks up loudly
/// through the banner (#offbar), which is the state that actually needs saying.
/// Kept as a no-op-when-absent writer so every caller stays unchanged.
export function setConn(html) {
  const el = $("#conn");
  if (el) el.innerHTML = html;
}

export function openAppSession({ preferDeviceId = null, waitForDevice = false } = {}) {
  return openRelaySession({
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    getPinnedDeviceKey: pinnedDeviceTransportKey,
    preferDeviceId,
    waitForDevice,
    isPaused: () => App.offline,
    onDeviceKey: markDeviceOnline,
    onDeviceOffline: markDeviceOffline,
    onLost: goOffline,
    // The bridge saying something moved. A frame nobody asked for reaches the
    // surface showing that state, which is what lets the polls stand down —
    // except the signaling pushes, which belong to the upgrade negotiating
    // them and describe nothing the surfaces show.
    onPush: (payload) => {
      if (!isSignaling(payload.type)) dispatchChangeEvent(payload);
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

let peerLink = null;

/**
 * Upgrade a live session onto a direct peer path, in the background.
 *
 * The user is already working over the relay: this fetches ICE servers, offers
 * over the relay carrier, and migrates both streams once the two channels are
 * open. A failure anywhere logs and leaves the session exactly where it is —
 * the relay is the fallback, not a retry target, so the next attempt is the
 * next relay session and nothing sooner.
 */
async function upgradeToPeer(session) {
  if (!globalThis.RTCPeerConnection) return;
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
  adoptPeerLink(session, link);
}

/** Put both streams on the connection that just opened — unless the session it
 *  was opened for is not the one the app is on any more. */
function adoptPeerLink(session, link) {
  if (App.session !== session) {
    link.close(); // a device switch overtook the upgrade
    return;
  }
  peerLink = link;
  // The two channels are one connection: whichever goes first takes the other,
  // and both streams migrate back to the relay together.
  for (const carrier of [link.app, link.term]) {
    carrier.onClose(() => {
      if (peerLink === link) dropPeerLink();
    });
  }
  session.peer(link.app);
  terminalsRideOn(link.term);
}

/** Idempotent, and the single point where both streams are handed back at once:
 *  nothing may end up on the relay while the other half still rides a peer
 *  connection that is going away. */
function dropPeerLink() {
  const link = peerLink;
  if (!link) return;
  peerLink = null;
  App.session?.peer(null);
  terminalsRideOn(null);
  link.close();
}

/** Greet a session that is live and unpaused: feature-detect push invalidation,
 *  subscribe this session to it, and read everything once. Not awaited by its
 *  callers — a slow greeting must not hold up the app, and a surface mounted
 *  before it lands is re-timed the moment it does. */
export function greetLiveBridge() {
  const session = App.session;
  const repository = App.chatRepository;
  if (!session) return Promise.resolve(false);
  return greetBridge(session.call, {
    isCurrent: () => App.session === session && App.chatRepository === repository,
    onGreeting: (greeting) => repository?.configureCapabilities(greeting),
  }).catch(() => {
    /* the session died mid-greeting; the next one greets again */
  });
}

export function adoptSession(session) {
  const deviceChanged = Boolean(App.cacheScope && App.cacheScope.deviceId !== session.deviceId);
  dropPeerLink(); // whatever was carrying was carrying the session we just left
  App.session = session;
  App.call = session.call;
  // Every later carrier change re-establishes the session on the wire it took:
  // session.hello, and a read of every mounted surface.
  session.onCarrier(greetLiveBridge);
  // Reconnects keep this device's controllers/drafts and only replace their
  // transport. A device switch retires the old scope before any new view can
  // capture it.
  adoptApplicationScope({ deviceId: session.deviceId, call: session.call });
  if (deviceChanged) resetFeedScope();
  paintDevicePicker();
  // Every live session starts here — the gate's first one, a reconnect, a
  // device switch — so this is where captures taken with no device to send them
  // to are handed over.
  flushCaptures().catch(() => {
    /* still unreachable: the queue keeps them for the next session */
  });
  upgradeToPeer(session); // in the background: the user is live already
}

function restoreOnline() {
  App.offline = false;
  App.offlineSince = null;
  document.body.classList.remove("offline");
  $("#offbar").hidden = true;
  setConn('<span class="dot"></span>connected');
}

let reconnectTimer = null;
let reconnectDelay = 0;

export function goOffline() {
  if (App.offline) return;
  App.offline = true;
  App.offlineSince = Date.now();
  dropPeerLink();
  try {
    App.session?.close();
  } catch {
    /* already gone */
  }
  document.body.classList.add("offline");
  const name = deviceName(App.session?.deviceId) || "Your device";
  $("#offbar-text").textContent = offlineBannerText(name, App.offlineSince);
  $("#offbar").hidden = false;
  setConn('<span class="dot" style="background:var(--amber)"></span>reconnecting…');
  resume();
}

export async function resume() {
  if (!App.offline || App._resuming) return;
  App._resuming = true;
  try {
    // Blocks until a device is online: the fresh authenticated socket receives
    // the relay's device_key push the moment a bridge returns. The sticky
    // choice is honored only when that device is online right now — otherwise
    // ANY of the user's devices brings us back (waiting on an offline sticky
    // device would discard the working device's return forever).
    const devices = await refreshDevices();
    const session = await openAppSession({
      preferDeviceId: onlineStickyDeviceId(devices, App.selectedDeviceId),
      waitForDevice: true,
    });
    if (!App.offline) {
      // Someone else (a device switch) already restored us while we waited.
      session.close();
      return;
    }
    adoptSession(session);
    restoreOnline();
    reconnectDelay = 0;
    // AFTER restoreOnline, which is what unpauses calls — and the greeting is a
    // call. Whatever happened while we were away was pushed at a socket that
    // was not there, so this reads every mounted surface once and re-arms event
    // mode on whatever bridge we came back to.
    greetLiveBridge();
    // The notifications view holds no user input, so refreshing it is safe; the
    // task view's own poll resumes and its key-diffing preserves in-progress work.
    if (App.route.name === "notifications") render();
  } catch {
    // Relay unreachable — retry with backoff until it's back.
    reconnectDelay = Math.min((reconnectDelay || 1000) * 2, 15000);
    clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(resume, reconnectDelay);
  } finally {
    App._resuming = false;
  }
}

/** Re-target the app at another device: open the new session first, then swap. */
export async function switchDevice(deviceId) {
  if (App.session?.deviceId === deviceId && !App.offline) {
    rememberSelectedDevice(deviceId);
    return;
  }
  const previous = App.session;
  const wasOffline = App.offline;
  App.offline = false; // let the fresh session's calls through
  let session;
  try {
    session = await openAppSession({ preferDeviceId: deviceId, waitForDevice: false });
  } catch (error) {
    App.offline = wasOffline;
    throw error;
  }
  try {
    previous?.close();
  } catch {
    /* already gone */
  }
  // Persist the sticky choice only once the switch actually succeeded — an
  // unreachable pick must not poison future boots/resumes.
  rememberSelectedDevice(deviceId);
  adoptSession(session);
  restoreOnline();
  // A different device is a different bridge: it may push where the last one
  // polled, or the other way round.
  greetLiveBridge();
  retargetTerminals(); // the terminal socket follows the app session's device
  render();
}
