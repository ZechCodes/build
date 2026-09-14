// The per-browser-tab terminal socket owner. ONE TerminalSocket multiplexes
// every terminal tab (user shells + agent screens) by term_id, kept off the app
// RPC session so PTY floods never head-of-line-block RPCs. Created lazily on the
// first terminal/agent tab mount — users who never open one pay no socket.

import * as transport from "@build/secure-transport";
import { RELAY_URL } from "../config.js";
import { App } from "../app.js";
import { fetchGatewayToken } from "../api.js";
import { pinnedDeviceTransportKey } from "../devices.js";
import { homeDeviceId } from "../core/devicePolicy.js";
import { canAnswer, contextFor } from "../core/deviceContexts.js";
import { TerminalSocket } from "./session.js";
import { createStatusHub } from "./statusHub.js";

let socket = null;
// The `term` DataChannel, once the app session's upgrade has one. Remembered at
// module scope because the socket is lazy: an upgrade can land long before the
// first terminal tab mounts one.
let peerCarrier = null;

// One hub for the whole page: the singleton socket's status feeds it, every pane
// overlay subscribes to it. Lives at module scope so subscribeTerminalStatus
// works even before the first pane mounts the socket.
const statusHub = createStatusHub();

export { createStatusHub };

/**
 * The machine the shells type at.
 *
 * A terminal belongs to the work on screen, so a link that names a machine this
 * client knows names the machine the socket is on — knows, not can reach right
 * now. A machine that has answered once owns the shells on its own surfaces
 * through an outage: moving them to another machine under a link about this one
 * would type the work at the wrong computer, and term ids are minted per
 * machine, so the tabs would come back pointing at somebody else's shells. They
 * stay, and the socket's own reconnect lands them again when the machine does.
 *
 * A surface about nowhere in particular — the inbox, an account page — leaves
 * the shells on the home device, which is where creation goes, and so does a
 * link naming a machine this client has never opened: that one mounts a notice
 * rather than a surface, and there is nothing yet to type at. Asked here by
 * everyone: nowhere else works out which machine that is.
 *
 * App is read lazily (app.js imports this module through connection.js), and so
 * is the device list: it is patched live by the relay's pushes.
 */
export function terminalDeviceId() {
  const routeDevice = contextFor(App.route?.deviceId);
  return routeDevice ? routeDevice.deviceId : homeDeviceId(App.devices, App.selectedDeviceId);
}

/** Subscribe to the terminal socket's connectivity status. The callback fires
 *  immediately with the current status if one is already known. Returns an
 *  unsubscribe function. */
export function subscribeTerminalStatus(fn) {
  return statusHub.subscribe(fn);
}

export function terminalManager() {
  if (!socket) {
    socket = new TerminalSocket({
      url: RELAY_URL,
      transport,
      WebSocketImpl: WebSocket,
      getToken: fetchGatewayToken,
      getPinnedDeviceKey: pinnedDeviceTransportKey,
      // Re-read on every reconnect; a route change or a home move calls
      // followTerminalDevice() to force that reconnect.
      preferDeviceId: terminalDeviceId,
    });
    socket.onStatus((status) => statusHub.set(status));
    // A failed initial connect self-heals: _connect closes the socket, whose
    // close event schedules the backoff reconnect. A channel that is already
    // carrying takes over once the session it re-attaches over exists.
    socket
      .start()
      .then(() => socket.peer(peerCarrier))
      .catch(() => {});
  }
  return socket;
}

/**
 * Ride the peer connection's `term` channel from now on, or `null` to fall back
 * to the relay socket. The app session's upgrade owns both channels, so this is
 * how the terminal stream learns that its half is open — and, when the peer path
 * goes, that it is back on the relay.
 *
 * Nothing is watched here. "The two channels are one connection and fall back
 * together" is written in `connection.js`, which hears each channel's close and
 * hands both streams back at once; a second listener on the same carrier would
 * run that fallback twice, through two owners of one fact.
 */
function terminalsRideOn(carrier) {
  peerCarrier = carrier || null;
  socket?.peer(peerCarrier);
}

/**
 * Re-point the terminal socket at the device the terminals follow, and say
 * whether the shells are there. A healthy socket never reconnects on its own —
 * the liveness ping keeps it pinned to the old device — so a move must drop it;
 * the auto-reconnect then re-reads preferDeviceId, attaches to the wanted
 * device, and re-attaches every open tab.
 *
 * That re-attach is why a machine that cannot answer takes nothing: every tab
 * would come back against a machine with no session to open a PTY on. The
 * shells stay where they are, and the caller asks again when the machine can.
 */
function retargetTerminals(wantedDeviceId) {
  if (!socket || socket.deviceId === wantedDeviceId) return true;
  if (!canAnswer(contextFor(wantedDeviceId))) return false;
  socket.simulateDrop();
  return true;
}

/**
 * Take the terminals to the device they now follow: drop a socket that is still
 * pinned somewhere else so it comes back on the right one, and ride that
 * device's peer channel if it has one (and nobody else's — another device's
 * channel carries the stream to the wrong machine).
 *
 * Every way the answer changes ends here: a route change (app.js render), a
 * home move, and a peer link opening or closing on that device. Answers whether
 * the shells took the move: a machine that cannot answer keeps neither the
 * socket nor the carrier, and the shells stay on the machine they are on.
 */
export function followTerminalDevice() {
  const deviceId = terminalDeviceId();
  if (!retargetTerminals(deviceId)) return false;
  terminalsRideOn(contextFor(deviceId)?.peerLink?.term || null);
  return true;
}
