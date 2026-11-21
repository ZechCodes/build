// One terminal session follows the browser tab's selected device. A capable
// HTTP Opening supplies WSS on demand. The RTC path requests its existing
// independently authenticated mint and rides the device's terminal channel.

import * as transport from "@build/secure-transport";
import { App } from "../app.js";
import { homeDeviceId } from "../core/devicePolicy.js";
import { canAnswer, contextFor } from "../core/deviceContexts.js";
import { createTerminalFollowController } from "./followController.js";
import { TerminalSocket } from "./session.js";
import { WebSocketTerminalSession } from "./webSocketSession.js";
import { isProjectTerminalScope, projectTerminalRpc } from "../core/projectTerminalRpc.js";
import { directoryCacheId } from "../core/directoryScope.js";
import { mergeCachedRecordsTogether, readCached, recordWriteOf } from "../core/localCache.js";
import { createStatusHub } from "./statusHub.js";
import { terminalPeerLink, terminalWebSocketOwner } from "../core/terminalPeerLinks.js";

let manager = null;

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

function scopedTerminalCall(deviceId, method, params, call) {
  return isProjectTerminalScope(params)
    ? projectTerminalRpc(contextFor(deviceId), method, params, call) : call(method, params);
}

async function refreshTerminalRoster(deviceId, scope, entityId = directoryCacheId(scope)) {
  const context = contextFor(deviceId);
  const session = context?.session;
  if (!session || !entityId) return;
  const address = { deviceId, entityId, kind: "terminals" };
  const before = recordWriteOf(await readCached(address));
  const stands = () => contextFor(deviceId) === context && context.session === session;
  const result = await scopedTerminalCall(deviceId, "term.list", scope, (method, params) => {
    if (!stands()) throw new Error("The terminal device changed before refreshing its roster.");
    return context.rpc(method, params);
  });
  await mergeCachedRecordsTogether([address], ([record]) =>
    stands() && recordWriteOf(record) === before ? [{ tabs: result.terminals || [] }] : null);
}

// Panes retain this interface while the manager follows a different transport.
function terminalActions(session) {
  return {
    get deviceId() { return session().deviceId; },
    whenConnected: (...args) => session().whenConnected(...args),
    listTerminals: (...args) => session().listTerminals(...args),
    createTerminal: (...args) => session().createTerminal(...args),
    closeTerminal: (...args) => session().closeTerminal(...args),
    attachTerminal: (...args) => session().attachTerminal(...args),
    attachAgent: (...args) => session().attachAgent(...args),
    detach: (...args) => session().detach(...args),
    input: (...args) => session().input(...args),
    resize: (...args) => session().resize(...args),
  };
}

function createTerminalManager() {
  const rtc = new TerminalSocket({
    transport,
    userTerminalDevice: terminalDeviceId,
    userTerminalRpc: scopedTerminalCall,
    onTermUnresponsive: () => followTerminalDevice({ freshSession: true }),
  });
  const rtcFollow = createFollowController(rtc);
  let active = rtc;

  function observe(session) {
    session.onStatus((status) => {
      if (manager === owned && active === session) statusHub.set(status);
    });
  }

  function followWithWebSocket(context, deviceId) {
    const owner = terminalWebSocketOwner(context);
    const retain = active !== rtc && active.deviceId === deviceId && !context?.session;
    if (!owner && !retain) return false;
    if (active === rtc) {
      rtcFollow.reset();
      active = new WebSocketTerminalSession({
        userTerminalDevice: terminalDeviceId,
        userTerminalRpc: scopedTerminalCall,
        rosterCall: (id, method, params) => contextFor(id).rpc(method, params),
        onUncertainRoster: refreshTerminalRoster,
        panes: rtc.takePanesForDevice(deviceId),
      });
      observe(active);
    }
    active.follow(deviceId, owner);
    return true;
  }

  function followWithRtc(target) {
    if (active !== rtc) {
      active.close();
      active = rtc;
    }
    return rtcFollow.request(target);
  }

  const owned = {
    terminals: terminalActions(() => active),
    followWithWebSocket,
    followWithRtc,
    close() {
      try {
        rtcFollow.reset();
        if (active !== rtc) active.close();
      } finally {
        rtc.close();
      }
    },
  };
  observe(rtc);
  return owned;
}

export function terminalManager() {
  if (!manager) {
    manager = createTerminalManager();
    followTerminalDevice();
  }
  return manager.terminals;
}

/** The exact terminal peer owner for this machine. HTTP app sessions acquire
 * their independent RTC parent lazily; ordinary RTC sessions reuse their peer. */
const peerLinkOf = (deviceId) => terminalPeerLink(contextFor(deviceId));

function stillDesired(target) {
  return terminalDeviceId() === target.deviceId
    && contextFor(target.deviceId) === target.context
    && canAnswer(target.context)
    && desiredPeer(target);
}

const desiredPeer = (target) => target.peerLink
  ? peerLinkOf(target.deviceId) === target.peerLink
  : (peerLinkOf(target.deviceId)?.term || null) === target.carrier;

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
  if (!manager) return answerable;
  if (manager.followWithWebSocket(context, deviceId)) return answerable;
  const peerLink = terminalPeerLink(context);
  return manager.followWithRtc({
    deviceId,
    context,
    peerLink,
    carrier: peerLink?.term || null,
    recovery: peerLink?.recovery || null,
    canAnswer: answerable,
    freshSession,
  });
}

/** Invalidate the old account before its device teardown can re-enter follow. */
export function resetTerminalManager() {
  const previous = manager;
  manager = null;
  statusHub.clear();
  previous?.close();
}
