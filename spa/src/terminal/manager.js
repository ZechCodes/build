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
import { TerminalSocket } from "./session.js";
import { createStatusHub } from "./statusHub.js";

let socket = null;
// The `term` DataChannel of the device the terminals follow. Remembered at
// module scope because the socket is lazy: a peer link can land long before the
// first terminal tab mounts one.
let peerCarrier = null;

/** How a terminal session is minted on one device — through that device's
 *  rendezvous, which the connection layer owns. Until it is provided, nothing
 *  can type anywhere: a terminal session is not this module's to open. */
let mintTerminalSession = async () => null;
// Every decision to follow a device supersedes any mint still crossing the
// rendezvous. Device identity alone is not enough: a reconnect can replace a
// peer link on the same device while the old mint is still in flight.
let followGeneration = 0;
let pendingFollow = null;
let acknowledgedCarrier = null;

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
    socket = new TerminalSocket({ transport });
    socket.onStatus((status) => statusHub.set(status));
    // A session on the device the shells are to type at, and the channel that
    // device's peer link is already carrying, if it has one. Until both are
    // there the panes are shown connecting, which is what they are.
    followTerminalDevice();
    if (peerCarrier) socket.peer(peerCarrier);
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
function terminalsRideOn(carrier, session = null, { confirm = false } = {}) {
  peerCarrier = carrier || null;
  if (session) return socket?.adoptTerminalSession(session, peerCarrier, { confirm });
  socket?.peer(peerCarrier);
}

/**
 * Take the terminal socket to another machine, and say whether the shells took
 * the move. The move is a session: one minted on that device's rendezvous,
 * adopted by the socket, which re-attaches every open tab over that device's
 * channel.
 *
 * That re-attach is why a machine that cannot answer takes nothing: every tab
 * would come back against a machine with no session to open a PTY on. The
 * shells stay where they are, and the caller asks again when the machine can.
 */
function moveTerminalsTo(deviceId, generation) {
  if (!canAnswer(contextFor(deviceId))) return false;
  adoptSessionOn(deviceId, generation);
  return true;
}

/** The `term` channel one machine's peer link is carrying, or null when it has
 *  none. Nobody else's: another device's channel carries the stream to the
 *  wrong machine. */
const termChannelOf = (deviceId) => contextFor(deviceId)?.peerLink?.term || null;

/**
 * Mint a terminal session on one machine and give the socket that session and
 * that machine's channel, together.
 *
 * Together is the whole of it. A socket handed the new machine's wire while it
 * still holds the old machine's session re-attaches THAT session's terminals
 * over a bridge which has never heard of it; a socket handed the new machine's
 * session and left to re-take the wire it is on attaches the new session over
 * the old machine's channel. Either way one of the two belongs to the machine
 * the shells have left, and a socket with nothing registered reports itself
 * connected on it.
 *
 * Not awaited: `followTerminalDevice` answers a route change, which cannot wait
 * on a relay round trip. A mint that fails leaves the shells on the session and
 * the wire they are on — the device it failed for is blocked by the layer that
 * owns its rendezvous, which is what the panes end up showing — and a mint that
 * lands after the shells have moved on again is dropped, because the session it
 * carries is the wrong machine's.
 */
function adoptSessionOn(deviceId, generation) {
  Promise.resolve()
    .then(() => mintTerminalSession(deviceId))
    .then((session) => acceptMintedSession(session, deviceId, generation))
    .catch(() => {
      clearPendingFollow(generation);
      /* that machine cannot mint one; the shells stay where they are */
    });
}

const clearPendingFollow = (generation) => {
  if (pendingFollow?.generation === generation) pendingFollow = null;
};

const currentFollowIs = (deviceId, generation) =>
  Boolean(socket && generation === followGeneration && terminalDeviceId() === deviceId);

async function acceptMintedSession(session, deviceId, generation) {
  if (!session) return clearPendingFollow(generation);
  try {
    if (!currentFollowIs(deviceId, generation)) return;
    await terminalsRideOn(termChannelOf(deviceId), session, { confirm: true });
    if (generation === followGeneration) acknowledgedCarrier = termChannelOf(deviceId);
  } catch {
    // A terminal session that was not acknowledged is not allowed to tear down
    // the app half of the shared peer.
    if (socket && generation === followGeneration) socket.peer(null);
  } finally {
    session.release?.();
    clearPendingFollow(generation);
  }
}

function beginFreshFollow(deviceId, carrier, freshSession) {
  const generation = ++followGeneration;
  pendingFollow = { deviceId, carrier, generation };
  if (freshSession) socket.peer(null);
  const moving = moveTerminalsTo(deviceId, generation);
  if (!moving) clearPendingFollow(generation);
  return moving;
}

const alreadyFollowing = (deviceId, carrier) =>
  pendingFollow?.deviceId === deviceId && pendingFollow.carrier === carrier;

const needsFreshFollow = (deviceId, carrier, freshSession) =>
  Boolean(socket && (socket.deviceId !== deviceId || freshSession || (carrier && carrier !== acknowledgedCarrier)));

function keepCurrentFollow(deviceId) {
  if (pendingFollow) {
    followGeneration += 1;
    pendingFollow = null;
  }
  terminalsRideOn(termChannelOf(deviceId));
  return true;
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
  const carrier = termChannelOf(deviceId);
  if (alreadyFollowing(deviceId, carrier)) return true;
  // Another machine: the session and that machine's wire go over together when
  // the mint lands. The shells keep the channel they are riding until then.
  return needsFreshFollow(deviceId, carrier, freshSession)
    ? beginFreshFollow(deviceId, carrier, freshSession)
    : keepCurrentFollow(deviceId);
}
