// The per-browser-tab terminal socket owner. ONE TerminalSocket multiplexes
// every terminal tab (user shells + agent screens) by term_id, kept off the app
// RPC session so PTY floods never head-of-line-block RPCs. Created lazily on the
// first terminal/agent tab mount — users who never open one pay no session.
//
// It owns no socket of its own (spec rule 5): the terminals ride the `term`
// DataChannel of the device they follow, on a session minted through that
// device's rendezvous. Minting one is the connection layer's — it owns the
// rendezvous — and is handed to this module as `mintTerminalSession`.

import * as transport from "@build/secure-transport";
import { App } from "../app.js";
import { homeDeviceId } from "../core/devicePolicy.js";
import { canAnswer, contextFor } from "../core/deviceContexts.js";
import { createTerminalFollowController } from "./followController.js";
import { TerminalSocket } from "./session.js";
import { createStatusHub } from "./statusHub.js";

let socket = null;
let followController = null;

/** How a terminal session is minted on one device — through that device's
 *  rendezvous, which the connection layer owns. Until it is provided, nothing
 *  can type anywhere: a terminal session is not this module's to open. */
let mintTerminalSession = async () => null;

/** Hand this module the mint. Called once, by the layer that owns the
 *  rendezvous of every device. */
export function provideTerminalSessions(mint) {
  mintTerminalSession = mint;
}

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
 * is the device list: it is refreshed live by the presence poll.
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
    // A terminal session that stopped answering on a wire the peer is still
    // carrying is a session to re-establish, not a connection to tear down:
    // the shells come back on the same channel, and the app session beside
    // them never hears about it.
    const created = new TerminalSocket({
      transport,
      onTermUnresponsive: () => followTerminalDevice({ freshSession: true }),
    });
    socket = created;
    created.onStatus((status) => {
      if (socket === created) statusHub.set(status);
    });
    followController = createFollowController(created);
    // A session on the device the shells are to type at, and the channel that
    // device's peer link is already carrying, if it has one. Until both are
    // there the panes are shown connecting, which is what they are.
    followTerminalDevice();
  }
  return socket;
}

/**
 * Ride this device's `term` channel from now on, or `null` for "there is no
 * wire". The device's peer link owns both channels, so this is how the terminal
 * stream learns that its half is open — and, when the peer path goes, that
 * there is nothing left to type down (rule 2: no relay fallback).
 *
 * `session` is that same device's freshly minted terminal session when the wire
 * comes with one, which is what a move to another machine is. The two go over
 * together on purpose: a socket given the session and left to pick its own wire
 * picks the one it is already riding, which is the machine the shells just
 * left.
 *
 * Nothing is watched here. "The two channels are one connection and go
 * together" is written in `connection.js`, which hears each channel's close and
 * hands both streams back at once; a second listener on the same carrier would
 * run that fallback twice, through two owners of one fact.
 */
/** The `term` channel one machine's peer link is carrying, or null when it has
 *  none. Nobody else's: another device's channel carries the stream to the
 *  wrong machine. */
const termChannelOf = (deviceId) => contextFor(deviceId)?.peerLink?.term || null;

function stillDesired(target) {
  return terminalDeviceId() === target.deviceId
    && contextFor(target.deviceId) === target.context
    && canAnswer(target.context)
    && termChannelOf(target.deviceId) === target.carrier;
}

function createFollowController(owner) {
  return createTerminalFollowController({
    mint: (deviceId) => mintTerminalSession(deviceId),
    adopt: (session, carrier, isCurrent, recovery) => owner.adoptTerminalSession(session, carrier, {
      confirm: true,
      isCurrent,
      recovery,
    }),
    ride: (carrier, recovery) => owner.peer(carrier, { recovery }),
    detach: () => owner.peer(null),
    isDesired: stillDesired,
  });
}

/**
 * Take the terminals to the device they now follow: a session on that machine
 * for a socket that is still on another one, and that device's peer channel if
 * it has one (and nobody else's — another device's channel carries the stream
 * to the wrong machine).
 *
 * Every way the answer changes ends here: a route change (app.js render), a
 * home move, and a peer link opening or closing on that device. Answers whether
 * the shells took the move: a machine that cannot answer keeps neither the
 * socket nor the carrier, and the shells stay on the machine they are on.
 */
export function followTerminalDevice({ freshSession = false } = {}) {
  const deviceId = terminalDeviceId();
  const context = contextFor(deviceId);
  const answerable = canAnswer(context);
  if (!socket) return answerable;
  return followController.request({
    deviceId,
    context,
    carrier: context?.peerLink?.term || null,
    recovery: context?.peerLink?.recovery || null,
    canAnswer: answerable,
    freshSession,
  });
}

/** Invalidate the old account before its device teardown can re-enter follow. */
export function resetTerminalManager() {
  const previousController = followController;
  const previous = socket;
  socket = null;
  followController = null;
  statusHub.clear();
  try {
    previousController?.reset();
  } finally {
    previous?.close();
  }
}
