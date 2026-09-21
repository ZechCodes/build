// Session lifecycle: one session per device, carried by that device's own
// direct connection — open, land, block, retry — and which of them the app
// calls home.
//
// The browser is live only over the DataChannels (spec rule 2). A device's
// rendezvous is how that device is found and its sessions minted
// (core/rendezvous.js); it carries `rtc.*` and nothing else, and closes the
// moment both channels are open (rule 4). There is no relay underneath, so a
// device whose connection cannot be made is blocked by name, with a reason
// (rule 3): its rows stay in the rail, greyed, its scope and drafts survive,
// its calls are refused in those words, and recovery runs the sequence again
// while authoritative presence still calls the device online.
//
// The account-wide waiting screen is reserved for authoritative presence saying
// every machine is offline. Online machines recover behind the existing shell.

import * as transport from "@build/secure-transport";
import { RELAY_URL } from "./config.js";
import { createRelayRendezvous } from "./core/rendezvous.js";
import { createRendezvousLifecycle } from "./core/rendezvousLifecycle.js";
import { createDeviceConnectionAttempts } from "./core/deviceConnectionAttempts.js";
import { createDeviceRecoverySupervisor } from "./core/deviceRecovery.js";
import { openSession } from "./core/session.js";
import { openPeerLink } from "./core/peerLink.js";
import { connectionDiagnosticHistory, recordConnectionDiagnostic } from "./core/connectionDiagnostics.js";
import { isSignaling } from "./core/sessionSwitch.js";
import { fetchGatewayToken, fetchIceServers } from "./api.js";
import { App, rememberSelectedDevice } from "./app.js";
import {
  adoptBridgeSelection,
  adoptDeviceConnection,
  announceDeviceTransport,
  blockCurrentDevice,
  blockDeviceConnection,
  canAnswer,
  clearDeviceSecurityStops,
  closeQuietly,
  contextFor,
  deviceContextEra,
  deviceContextIdentity,
  deviceSecurityStopText,
  existingDeviceLifecycle,
  greetingToken,
  homeContext,
  knownContexts,
  knownDeviceContext,
  liveContexts,
  loseDeviceConnection,
  markDevicePresenceAway,
  onDeviceStateChanged,
  refuseDeviceConnection,
  releaseGreeting,
  retryDeviceConnection,
  resetDeviceContexts,
  retireDeviceContext,
} from "./core/deviceContexts.js";
import { deviceAwayText } from "./core/deviceAway.js";
import { deviceNameOf, listingCouldBeLagging } from "./core/devicePolicy.js";
import { pinnedDeviceTransportKey } from "./devices.js";
import { followTerminalDevice, provideTerminalSessions, terminalDeviceId } from "./terminal/manager.js";
import { LIVENESS_TIMEOUT } from "./terminal/session.js";
import { flushCaptures } from "./core/composeView.js";
import { dispatchChangeEvent, greetBridge } from "./core/changeEvents.js";
import { deliverFeed, joinFeed } from "./core/taskFeed.js";

// ---- one rendezvous per device (spec rules 4, 5 and 7) -----------------------

// How each machine is found, one rendezvous per device context. The map owns
// them because a machine can be dialled before it has a context — a connect
// that fails never lands one — and the context carries the same object, so
// anything holding a device can find the way to it.
/** The way to one machine, opened on demand by whatever needs it. A relay
 *  socket today; a direct-network listener is a second implementation of the
 *  same interface, and nothing below this line would know (rule 7). */
const rendezvousLifecycle = createRendezvousLifecycle((deviceId) =>
  createRelayRendezvous({
    deviceId,
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    // The api-pinned transport key, never a relay-supplied one: the relay is an
    // untrusted broker and session keys are sealed exclusively to that key.
    getPinnedDeviceKey: pinnedDeviceTransportKey,
  }),
);

// One explicit attempt owner per device. This stage owns only the attempt and
// its provisional resources; deviceContexts remains the availability model.
const connectionAttempts = createDeviceConnectionAttempts();

// Recovery decides when another attempt may begin; connectionAttempts remains
// the sole owner of the attempt and every provisional transport it opens.
const deviceRecovery = createDeviceRecoverySupervisor({
  attempt: (deviceId, epoch) => attemptRecoveryDevice(deviceId, epoch),
  cancelAttempt: (deviceId) => connectionAttempts.forDevice(deviceId).cancel(),
});

/**
 * What the app listens to for "the reason the last dial failed has probably gone".
 *
 * Three signals, none of which is evidence about the machine on the other end,
 * all of which mean the wait is now pointless: the screen came back, the network
 * came back, the radio changed. A phone asleep for twenty minutes climbs the
 * back-off ladder out of its OWN absence, and Zech's was still sitting on the top
 * step when he picked it up — half a minute of nothing on a device that was ready
 * to connect (issue #60).
 *
 * `visibilitychange` is the one that matters most and the one the app already had
 * for other purposes; `online` and the Network Information `change` are cheap and
 * cover a radio handover the screen never noticed. Each is recorded, so a report
 * carries the wake as well as the dials it caused.
 */
const WAKE_SIGNALS = Object.freeze([
  ["visible", () => document.visibilityState !== "hidden"],
  ["online", () => true],
  ["network-change", () => true],
]);

let stopWakeWatch = null;

/** Retry every waiting device now, from the floor, because something changed. */
function wakeRecovery(reason) {
  const woken = deviceRecovery.wake(reason);
  // Only worth a line when it did something: a visibility flip on a healthy app
  // happens all day and says nothing.
  if (woken.woke.length) {
    recordConnectionDiagnostic("recovery", "woken", { reason, devices: woken.woke.length });
  }
  return woken;
}

/**
 * Listen for the three wake signals while the app is up. Re-entrant: asking
 * again re-arms the one set rather than adding a second.
 */
export function watchForWake() {
  stopWatchingForWake();
  const [visible] = WAKE_SIGNALS;
  const onVisible = () => {
    if (visible[1]()) wakeRecovery("visible");
  };
  const onOnline = () => wakeRecovery("online");
  const onNetworkChange = () => wakeRecovery("network-change");
  // `navigator.connection` is absent on Safari and on desktop Firefox; a missing
  // signal costs nothing here because the other two still fire.
  const radio = globalThis.navigator?.connection;
  document.addEventListener("visibilitychange", onVisible);
  globalThis.addEventListener?.("online", onOnline);
  radio?.addEventListener?.("change", onNetworkChange);
  stopWakeWatch = () => {
    document.removeEventListener("visibilitychange", onVisible);
    globalThis.removeEventListener?.("online", onOnline);
    radio?.removeEventListener?.("change", onNetworkChange);
  };
}

/** Stop listening (the gate took the app back, the account signed out). */
export function stopWatchingForWake() {
  stopWakeWatch?.();
  stopWakeWatch = null;
}

export const syncDeviceRecoveryPresence = (devices) => deviceRecovery.syncPresence(devices);
export const deviceRecoverySnapshot = (deviceId) => deviceRecovery.snapshot(deviceId);
export const onDeviceRecoveryChanged = (listener) => deviceRecovery.subscribe(listener);

/** Close this machine's rendezvous whoever was holding it: the device is
 *  blocked, or gone, and nothing is negotiating with it any more. */
function closeRendezvous(deviceId) {
  rendezvousLifecycle.forDevice(deviceId).forceClose();
}

/** Let go of every attempt and rendezvous (sign-out, teardown): they are this
 *  account's, and none may land or remain open after the account has gone. */
export function forgetRendezvousSockets() {
  deviceRecovery.reset();
  connectionAttempts.clear();
  resetDeviceContexts();
  rendezvousLifecycle.clear();
}

// ---- the connect sequence (spec rules 2 and 3) -------------------------------

/** Name why a device could not be reached, on the error that says so. The first
 *  answer wins: the layer nearest the failure knows best what it was. */
const becauseOf = (error, reason) => Object.assign(error, { blockedReason: error?.blockedReason || reason });

/** The same, for a step that is awaited rather than thrown in place. */
const failingAs = (reason, pending) =>
  pending.catch((error) => {
    throw becauseOf(error, reason);
  });

/** Which of rule 3's words this failure gets. Everything names itself on the
 *  way out; anything that did not is the connection failing. */
const reasonOf = (error) => error?.blockedReason || "failed";

/**
 * One device's E2EE session, minted over that device's rendezvous.
 *
 * Every rendezvous failure means the same thing — the bridge is not there to be
 * found — except a key that is not the one this account pinned, which is not an
 * outage at all: the machine answering is not the machine that was paired.
 */
async function mintAppSession(deviceId, rendezvous, authority) {
  try {
    return await openSession({
      rendezvous,
      transport,
      deviceId,
      // One device's offline state pauses one device's calls.
      isPaused: () => contextFor(deviceId)?.offline === true,
      // The only carrier this session ever had has gone (rule 3's `lost`).
      onLost: () => {
        if (authority.current()) authority.lose();
      },
      // The bridge saying something moved. A frame nobody asked for reaches the
      // surfaces showing that device's state — except the signaling pushes,
      // which belong to the upgrade negotiating them.
      onPush: (payload) => {
        if (authority.current() && !isSignaling(payload.type)) dispatchChangeEvent(payload, deviceId);
      },
    });
  } catch (error) {
    throw becauseOf(error, error?.securityCritical ? "refused" : "unreached");
  }
}

/**
 * This device's direct connection: the one carrier it will have.
 *
 * `rtc.*` rides the rendezvous — the session's own routing rule — and the two
 * cues below are what keeps the relay open only while something is negotiating:
 * the channels opening let it go, and a connection that failed asks for it back
 * before it offers the restart.
 */
function openDirectLink(deviceId, session, sessionLease, authority) {
  let restartLease = null;
  return openPeerLink({
    signal: (method, params) => failingAs("refused", session.call(method, params)),
    fetchIceServers: () => failingAs("ice-servers", fetchIceServers()),
    onPush: session.onPush,
    diagnosticId: `${deviceId}:${session.sessionId}`,
    onConnected: () => {
      restartLease?.release();
      restartLease = null;
    },
    onFailed: async () => {
      if (!authority.current()) throw new Error(`stale rendezvous restart for ${deviceId}`);
      const lease = sessionLease.reacquire();
      if (!lease) throw new Error(`stale rendezvous restart for ${deviceId}`);
      restartLease = lease;
      try {
        await session.reattachSignaling();
      } catch (error) {
        lease.release();
        if (restartLease === lease) restartLease = null;
        throw error;
      }
    },
  });
}

// Intentionally content-free and bounded; support can ask a user to run this
// after a failure without requiring the console to have been open beforehand.
globalThis.buildConnectionDiagnostics = connectionDiagnosticHistory;

/**
 * Find this machine, open its connection, and put the session on it.
 *
 * `stillWanted` is asked once more with the connection open and before anything
 * is landed: a dial runs for as long as a relay round trip and two handshakes,
 * and the account can let the machine go in that time. What this dial opened is
 * this dial's to close — and nothing else is, because by then the machine may
 * have been handed back and a newer dial may have landed a session at it.
 */
async function connectOverChannels(deviceId, attempt) {
  if (!globalThis.RTCPeerConnection) {
    throw becauseOf(new Error("this browser cannot open a direct connection"), "no-webrtc");
  }
  const sessionLease = rendezvousLifecycle.forDevice(deviceId).acquire();
  let session;
  let link;
  let lifetime = null;
  const contextEra = deviceContextEra();
  const contextIdentity = deviceContextIdentity(deviceId);
  const authority = {
    current: () => deviceContextEra() === contextEra
      && deviceContextIdentity(deviceId) === contextIdentity
      && (lifetime?.current() ?? attempt.isCurrent()),
    lose: () => lifetime && loseEstablishedConnection(deviceId, lifetime),
  };
  try {
    session = await mintAppSession(deviceId, sessionLease.rendezvous, authority);
    if (!authority.current()) {
      closeQuietly(session);
      throw new Error(`connection attempt for ${deviceId} was cancelled`);
    }
    if (!attempt.own(session, closeQuietly)) throw new Error(`connection attempt for ${deviceId} was cancelled`);
    link = await openDirectLink(deviceId, session, sessionLease, authority);
    // The session's path probe stands down while the link is putting the path
    // right itself (#30): a restart keeps the channels open and owns its own
    // verdict, and two things judging one path reach it twice.
    session.watchRecovery(() => link.recovery.snapshot().recovering);
    // The ring says which way each machine is carrying, read live off the link
    // (connectionStatus.js). It repaints on availability and on recovery, and a
    // re-nomination is neither — so a path that changed under a steady session
    // would otherwise keep showing the old word until something unrelated
    // redrew it (#31).
    link.onPathChanged(announceDeviceTransport);
    if (!attempt.own(link, (owned) => owned.close())) {
      throw new Error(`connection attempt for ${deviceId} was cancelled`);
    }
    if (!authority.current()) throw new Error(`connection attempt for ${deviceId} was cancelled`);
    const landed = await landSession(session, link, sessionLease.release, attempt, authority, (adopted) => {
      lifetime = adopted;
      attempt.release(session);
      attempt.release(link);
      attempt.own(adopted, () => loseEstablishedConnection(deviceId, adopted));
    });
    if (!attempt.isCurrent()) throw new Error(`connection attempt for ${deviceId} was cancelled`);
    attempt.release(lifetime);
    return landed.context;
  } finally {
    sessionLease.release();
  }
}

/**
 * Connect one device and land it (spec rule 3's sequence).
 *
 * Any failure blocks that device: its rendezvous and session close, its link is
 * dropped, its rows grey with the reason, and whatever was queued for a wire is
 * refused in those words. Nothing falls back to the relay. Recovery, the Retry
 * controls, and presence when the machine comes back all call this same one
 * attempt owner.
 */
export function connectDevice(deviceId, { recoveryEpoch = null } = {}) {
  handTerminalsTheirMint();
  const attemptOwner = connectionAttempts.forDevice(deviceId);
  // Callers converge on the attempt owner's exact promise. Only the caller that
  // starts it reports its outcome to recovery, or one failure would advance the
  // backoff once per observer rather than once per dial.
  if (attemptOwner.connecting) return attemptOwner.connect(() => null);
  const epoch = recoveryEpoch ?? deviceRecovery.beginAttempt(deviceId);
  const identity = deviceContextIdentity(deviceId);
  const pending = attemptOwner.connect(
    (attempt) => connectOnce(deviceId, attempt),
    { onFailure: (error) => {
      if (deviceContextIdentity(deviceId) === identity) connectionFailed(deviceId, error);
    } },
  );
  pending.then(
    () => deviceRecovery.connected(deviceId, { epoch }),
    (error) => deviceRecovery.failed(deviceId, { epoch, retryable: retryableConnectionFailure(deviceId, error) }),
  );
  return pending;
}

function attemptRecoveryDevice(deviceId, epoch) {
  retryDeviceConnection(deviceId);
  connectDevice(deviceId, { recoveryEpoch: epoch }).catch(() => {});
}

function retryableConnectionFailure(deviceId, error) {
  const lifecycle = existingDeviceLifecycle(deviceId)?.snapshot();
  if (lifecycle?.securityStop || contextFor(deviceId)?.unsupported) return false;
  return !error?.securityCritical && error?.blockedReason !== "no-webrtc";
}

async function connectOnce(deviceId, attempt) {
  // Barred for good: offering the same pinned key to the same impostor again
  // would neither fix that nor tell anyone about it.
  const securityStop = existingDeviceLifecycle(deviceId)?.snapshot().securityStop;
  if (securityStop) throw new Error(securityStop);
  return connectOverChannels(deviceId, attempt);
}

/** Apply only the failure the controller committed as authoritative. */
function connectionFailed(deviceId, error) {
  // A retry refused by the existing security stop did not try a connection.
  // Keep the original refusal rather than relabeling it as an ordinary outage.
  if (existingDeviceLifecycle(deviceId)?.snapshot().securityStop) return;
  closeRendezvous(deviceId);
  if (error?.securityCritical) refuseDeviceConnection(deviceId, error.message);
  else blockCurrentDevice(deviceId, reasonOf(error));
  syncHome(contextFor(deviceId));
}

/** One device's peer link opened or closed. The terminals move only when they
 *  are on that device — another device's channel carries the stream to the
 *  wrong machine, and nothing about the wire theirs rides has changed. */
function followTerminalsIfTheirs(context, options) {
  if (context.deviceId === terminalDeviceId()) followTerminalDevice(options);
}

/**
 * The terminal channel of an established connection went.
 *
 * It costs the whole connection only where it is evidence about the PATH: the
 * terminals judging that nothing carried anywhere (`LIVENESS_TIMEOUT`), or an
 * app channel that has gone with it. Anything else — a stream that wedged
 * under a loaded bridge, a chunk this browser could not reassemble — costs the
 * terminals their session and nothing more, and they take a fresh one on the
 * channel the peer is still carrying.
 */
function loseTerminalWire(deviceId, lifetime, reason) {
  // The connection has already gone — the app channel went with the path, and
  // its own handler is what said so.
  if (!lifetime.current()) return false;
  if (reason === LIVENESS_TIMEOUT) return loseEstablishedConnection(deviceId, lifetime);
  followTerminalsIfTheirs(contextFor(deviceId), { freshSession: true });
  return false;
}

function loseEstablishedConnection(deviceId, lifetime) {
  if (!lifetime.current()) return false;
  if (!connectionAttempts.isConnecting(deviceId)) closeRendezvous(deviceId);
  const { context, changed } = loseDeviceConnection(deviceId, lifetime);
  if (!changed) return false;
  syncHome(context);
  // Before connectOnce hands the lifetime off, its own rejection is the failed
  // fresh attempt and owns the backoff. Starting recovery here would supersede
  // that epoch and turn repeated greeting-time drops into a silent tight loop.
  if (!connectionAttempts.isConnecting(deviceId) && !context.unsupported) deviceRecovery.recoverNow(deviceId);
  return true;
}

/**
 * Mint the terminals a session on one machine (spec rule 5).
 *
 * It is minted through that machine's own rendezvous — the same socket its app
 * session was minted on, reopened if it had closed — and rides that machine's
 * `term` channel from then on. Nothing is held open for it: the socket closes
 * again the moment neither this mint nor a negotiation needs it.
 */
async function mintTerminalSession(deviceId) {
  const lease = rendezvousLifecycle.forDevice(deviceId).acquire();
  try {
    const minted = await lease.rendezvous.mint({});
    return {
      ...minted,
      release: lease.release,
    };
  } catch (error) {
    lease.release();
    throw error;
  }
}

// Handed to the terminals at the first connect rather than at module scope:
// this module and the manager import one another through the app shell, so a
// call written at the top level would run while the manager was still being
// evaluated. Nothing can follow a device before one has been connected, so
// this is early enough.
let terminalsKnowTheirMint = false;
function handTerminalsTheirMint() {
  if (terminalsKnowTheirMint) return;
  terminalsKnowTheirMint = true;
  provideTerminalSessions(mintTerminalSession);
}

// ---- blocking one device, and letting one go ---------------------------------

/**
 * This machine cannot be reached: block it, in rule 3's words for why.
 *
 * Its context keeps everything it was holding — the rows stay in the rail,
 * greyed, the scope and drafts survive — and loses only the connection: the
 * session is closed, the link dropped, the rendezvous closed. What was waiting
 * for a wire is refused in the same words the strip over its surfaces shows,
 * rather than held for a channel that is not coming.
 */
function blockDevice(deviceId, reason) {
  const attempts = connectionAttempts.forDevice(deviceId);
  if (!attempts.connecting) attempts.cancel();
  closeRendezvous(deviceId);
  blockCurrentDevice(deviceId, reason);
  const context = contextFor(deviceId);
  syncHome(context);
  if (!context.unsupported) deviceRecovery.recoverNow(deviceId);
}

/**
 * A live device's direct connection ended — the channels went and no ICE
 * restart brought them back. That is rule 3's `lost`, and the fast signal that
 * a machine is not there: the presence poll is the other, slower one.
 */
export function goOffline(deviceId) {
  const context = contextFor(deviceId);
  if (!context || context.offline) return;
  blockDevice(deviceId, "lost");
}

/**
 * The account no longer lists this machine as online (spec rule 6).
 *
 * Its bridge has gone, so it is plainly away rather than blocked: nothing here
 * failed to reach it, and there is nothing for a reader to retry until the api
 * says it is back. A machine that WAS blocked and has now gone is written over
 * for the same reason — and because a block is what keeps this layer from
 * asking for a machine again, so one that is only away is asked for the moment
 * the account calls it online.
 */
export function deviceWentAway(deviceId) {
  const context = contextFor(deviceId);
  if (!context || (context.offline && !context.blocked)) return;
  // A connect in flight owns that machine's rendezvous: standing the machine
  // down would close the socket the handshake is being made over and re-block
  // the machine that was about to answer — on a list that is up to a heartbeat
  // window out of date. The dial says how it went, and the next poll writes
  // what the account says over it.
  if (connectionAttempts.isConnecting(deviceId)) return;
  deviceRecovery.stop(deviceId, { cancel: true });
  const attempts = connectionAttempts.forDevice(deviceId);
  attempts.cancel();
  closeRendezvous(deviceId);
  markDevicePresenceAway(deviceId);
  // Home may have moved off it — and if it has not, the surfaces that follow
  // home still have to say the device they are about cannot be reached.
  syncHome(contextFor(deviceId));
}

/**
 * Let a device go for good: the account no longer has it.
 *
 * The registry forgets the machine and tells every surface standing over it,
 * but three things it is holding are this layer's: the direct connection it may
 * be riding — retiring through the registry alone would leave an
 * RTCPeerConnection open for the life of the tab — the rendezvous that finds
 * it, and the bar on a machine that was refused, which is not a bar on a
 * machine the account no longer has.
 */
export function retireDevice(deviceId) {
  // A dial in flight at this machine is called off first: whatever it lands or
  // fails at is about a machine the account no longer has.
  deviceRecovery.stop(deviceId, { cancel: false });
  connectionAttempts.retire(deviceId);
  // Closed first: this machine is not lost, it is gone, and nothing is to be
  // blocked on the way out.
  rendezvousLifecycle.retire(deviceId);
  return retireDeviceContext(deviceId);
}

/** What to say about a machine this client has stopped dialling, or "" while
 *  every machine is merely unreachable. A stop is the one connection failure a
 *  reader can act on and the only one that never resolves itself, so the
 *  screen holding the app says it out loud. */
export const securityStopText = deviceSecurityStopText;

/** Let go of every bar (sign-out, teardown): they are this account's, and the
 *  next account's machines have not been refused anything. */
export function forgetSecurityStops() {
  clearDeviceSecurityStops();
}

// ---- a settings page's view of a device --------------------------------------

/**
 * The session the app is already holding for one machine, for the page about
 * that machine.
 *
 * There is no second socket to open: the relay is a rendezvous and the bridge
 * refuses everything but `rtc.*` over it, so a page that wants to ask a machine
 * anything asks over the channel that machine's context already has. What it is
 * handed is a lease — closing it lets go of the watch, never of the machine's
 * own connection — and a machine that cannot answer is refused here, in the
 * same sentence its surfaces show.
 */
export async function openDeviceSettingsSession(deviceId, { onLost = () => {} } = {}) {
  const context = contextFor(deviceId);
  if (!canAnswer(context)) throw new Error(deviceAwayText(context, deviceNameOf(App.devices, deviceId)));
  const opened = context.session;
  let stopWatching = () => {};
  stopWatching = onDeviceStateChanged(() => {
    const now = contextFor(deviceId);
    if (canAnswer(now) && now.session === opened) return;
    stopWatching();
    onLost();
  });
  return {
    deviceId,
    // The device's own caller, not this session's: a reconnect replaces the
    // transport under a page that stays mounted, and every call is refused in
    // the account's words while the machine cannot answer.
    call: (...asked) => context.rpc(...asked),
    close: () => stopWatching(),
  };
}

// ---- greeting, landing, and which device is home -----------------------------

/** Greet a device that is live and unpaused: feature-detect push invalidation,
 *  subscribe that session to it, and read everything it has once. Not awaited by
 *  its callers — a slow greeting must not hold up the app, and a surface mounted
 *  before it lands is re-timed the moment it does.
 *
 *  Adopting the session armed this device's greeting (core/deviceContexts.js),
 *  and the selection this one settles is what releases it. A greeting that
 *  settles nothing — the session died before it said anything — is released
 *  here instead: the feed waits on that promise, and a machine whose greeting
 *  went missing must not be left unread for ever. */
export function greetLiveBridge(context, {
  suppressFailure = true,
  isAuthoritative = () => true,
  lifetime = null,
} = {}) {
  if (!isAuthoritative()) return Promise.resolve(false);
  const session = context?.session;
  if (!session) return Promise.resolve(false);
  const greetingAuthority = greetingToken(context);
  const repository = context.chatRepository;
  const greeting = greetBridge(session.call, {
    strict: !suppressFailure,
    deviceId: session.deviceId,
    // The device's context may have been retargeted onto a newer session while
    // this greeting was in flight; that greeting belongs to the session that
    // asked for it, not to the one the device is on now.
    isCurrent: () => isAuthoritative() && contextFor(session.deviceId)?.session === session,
    onGreeting: (greeting) => repository?.configureCapabilities(greeting),
    // The session first, so a gate that lets the app back in finds it there;
    // then the device's context, which is where every surface reads what this
    // machine's bridge speaks (core/deviceContexts.js).
    install: (selection) => {
      const adapter = session.installAdapter(selection);
      adoptBridgeSelection(context, selection, adapter);
      return adapter;
    },
  }).then((settled) => {
    // #30: a greeting that landed is the first moment a reconnect can settle
    // the posts the last session left uncertain — the repository outlived that
    // session, and the bridge's operation ledger can be asked again. Not
    // awaited: the app comes back the moment the bridge has said what it
    // speaks, and the recovery announces itself through the controllers when it
    // answers. `settled` passes through untouched.
    if (isAuthoritative() && contextFor(session.deviceId)?.session === session) {
      // Pictures first, and synchronously: the next repaint of a timeline the
      // reader is already looking at is what asks for them again, and it can be
      // milliseconds away.
      releaseDeferredAttachments(session.deviceId, repository);
      void repository?.resolveUncertainPosts();
    }
    return settled;
  }).catch((error) => {
    // Wake greeting-dependent reads only after this failed session is marked
    // unavailable; otherwise the feed can send a request between the rejected
    // hello and the outer connection attempt's cleanup.
    if (!suppressFailure && isAuthoritative() && contextFor(session.deviceId)?.session === session) {
      if (lifetime) blockDeviceConnection(session.deviceId, lifetime, reasonOf(error));
    }
    throw error;
  }).finally(() => {
      // Only this session's own: a newer one has armed a greeting of its own,
      // and what this one failed to say is no answer about that bridge.
      releaseGreeting(context, greetingAuthority);
    });
  return suppressFailure ? greeting.catch(() => false) : greeting;
}

/** Ask this device's conversations for the pictures a dead path ate (#30), and
 *  record that it happened.
 *
 *  Recorded because a figure that stays on "loading" after a reconnect has two
 *  possible causes — nothing released it, or nothing repainted after it was
 *  released — and Settings → Diagnostics is where a reader's report has to be
 *  able to tell them apart. Silent when there was nothing waiting, which is
 *  almost every reconnect. */
function releaseDeferredAttachments(deviceId, repository) {
  const released = repository?.retryDeferredAttachments();
  if (!released?.paths) return;
  recordConnectionDiagnostic(`${deviceId}:attachments`, "attachments-released", released);
}

/** Everything a device gets the moment it is live over its channels: the
 *  registry adopts it (a reconnect keeps that device's scope, drafts and
 *  controllers and only replaces its transport), it holds its connection, the
 *  bridge is greeted over that connection, and the feed starts reading it.
 *
 *  Nothing is dispatched to the user's surfaces before both channels are open
 *  (rule 2), so this is the first moment anything is asked of the machine. */
async function landSession(session, link, releaseInitialLease, attempt, authority, onAdopt) {
  if (!authority.current()) throw new Error(`connection attempt for ${session.deviceId} was cancelled`);
  let context = knownDeviceContext(session.deviceId);
  let lifetime;
  attempt.release(session);
  attempt.release(link);
  try {
    ({ context, lifetime } = adoptDeviceConnection(session, link, () => {
      session.peer(null);
      followTerminalsIfTheirs(context);
    }));
  } catch (error) {
    closeQuietly(session);
    link.close("the session could not be adopted");
    throw error;
  }
  onAdopt(lifetime);
  if (!lifetime.current()) throw new Error("session replaced during adoption");
  // The app channel IS the connection: it goes, the connection goes. The
  // terminal channel is one stream on the same path, and a path that died
  // takes both channels with it — so a term channel that goes ALONE is a
  // terminal stream to re-establish, not a device to drop. The one exception
  // is the terminals' own liveness judgement that nothing is carrying
  // anywhere, which says the path is dead in so many words.
  link.app.onClose(() => loseEstablishedConnection(session.deviceId, lifetime));
  link.term.onClose((reason) => loseTerminalWire(session.deviceId, lifetime, reason));
  // Every later carrier change re-establishes the session on the wire it took:
  // session.hello, and a read of every mounted surface.
  let initialGreeting = true;
  session.onCarrier(() => greetLiveBridge(context, {
    suppressFailure: !initialGreeting,
    isAuthoritative: () => lifetime.current(),
    lifetime,
  }));
  // Bind the app channel before releasing signaling. The carrier callback is
  // the acknowledged session.hello, so the relay cannot disappear in the gap
  // between WebRTC opening and the application session becoming usable.
  try {
    await session.peer(link.app);
  } finally {
    initialGreeting = false;
  }
  if (!attempt.isCurrent() || !authority.current()) {
    throw new Error(`connection attempt for ${session.deviceId} was cancelled`);
  }
  releaseInitialLease();
  // A device the feed is not polling yet — the account's first session, one a
  // late device just opened — gets its own board watcher and reads it as soon
  // as its greeting is in.
  joinFeed(context);
  syncHome(context);
  followTerminalsIfTheirs(context, { freshSession: true });
  return { context, lifetime };
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
  // The terminals ride the home device's channel unless a link names another
  // machine: home moving is one of the two things that moves them (a route
  // change is the other).
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
  // is how the picker and the composer come to say it has gone. At boot nothing
  // has been followed yet, which is how the device the pick names keeps home
  // while it is still handshaking.
  const home = homeContext() || contextFor(followed);
  if (home && (home.deviceId !== followed || home === landed)) followHomeContext(home);
}

// ---- opening every device ----------------------------------------------------

/**
 * Open every online device that has no live session, all at once.
 *
 * `first` is for a caller that needs A device — the gate, so the app starts on
 * whichever machine answers rather than on the slowest one; `settled` is every
 * context that came up. Nothing waits on the slowest device.
 *
 * This is also how a late device joins: the presence poll calls it after every
 * refresh (rule 6), and a machine that has just come online is one nothing is
 * holding a session for.
 *
 * `retry` is a READER asking — the waiting screen's account-wide Retry — rather
 * than the poll. A block is what keeps this layer from asking for a machine
 * again (rule 3), so it is the very thing a Retry undoes: every blocked machine
 * loses its mark here and is dialled, whatever the account list says about it.
 * The list is derived from a heartbeat and is up to a heartbeat window out of
 * date (rule 6), so the machines a reader is retrying are routinely ones it
 * still calls offline — asking only the ones it calls online would clear the
 * reason off every row and dial nobody.
 */
export function openDeviceSessions({ retry = false } = {}) {
  deviceRecovery.syncPresence(App.devices);
  const unblocked = retry ? askBlockedMachinesAgain() : [];
  if (retry) for (const deviceId of unblocked) deviceRecovery.beginAttempt(deviceId, { resetFailures: true });
  const wanted = App.devices.filter(wantsSession).map((device) => device.id);
  const attempts = dialEach([...unblocked, ...wanted]);
  const dials = attempts.length ? attempts : guessAtStaleDevices();
  return { first: handled(firstContext(dials)), settled: handled(everyContext(dials)) };
}

/** Dial each of these machines once, however many of the lists above named it. */
function dialEach(deviceIds) {
  return [...new Set(deviceIds)].map((deviceId) => connectDevice(deviceId));
}

/**
 * Every machine this client blocked is to be asked again, and the ids of the
 * ones whose mark was cleared.
 *
 * The mark that says not to ask is cleared before the dial, or the dial is
 * refused by the block the reader is undoing — and the ids come back because a
 * cleared machine MUST be dialled: the dial is what puts a reason back on its
 * row (the failure blocks it again, in whatever words it failed with this
 * time), so a clear with no dial leaves the row saying nothing at all.
 *
 * A machine this client has barred is not one of them: its key was not the key
 * this account pinned, dialling it again would offer the same key to the same
 * impostor, and `refused` is the standing answer rather than one attempt's.
 *
 * How long a machine has been away is not news a Retry changes, so it keeps the
 * moment it went; what it goes back to wearing is what this attempt finds out.
 */
function askBlockedMachinesAgain() {
  const cleared = [];
  for (const context of knownContexts()) {
    if (!context.blocked || existingDeviceLifecycle(context.deviceId)?.snapshot().securityStop) continue;
    retryDeviceConnection(context.deviceId);
    cleared.push(context.deviceId);
  }
  return cleared;
}

/**
 * With nothing else to try, dial the machines the account calls offline.
 *
 * The api's presence is derived from a heartbeat, so a bridge that came back
 * since the last one is listed offline for up to a minute and a half. An
 * account holding nothing would sit on that stale list while a machine was
 * answering, so each such machine is asked once, without waiting on it: it
 * either answers, or it is blocked with its reason and waits for the poll to
 * say it is back.
 *
 * Only the machines whose listing could actually be that far behind, though
 * (core/devicePolicy.js). The guess is a hedge against a window of lag, not a
 * licence to dial the whole account: a machine the api has not heard from in
 * days is off, and asking the relay for it only earns another
 * `rejected session to device`.
 */
function guessAtStaleDevices() {
  if (liveContexts().length) return []; // something is answering; the poll will hear the rest
  return App.devices.filter(worthGuessingAt).map((device) => connectDevice(device.id));
}

/** A machine nobody has asked for, whose listing this call has a reason to
 *  doubt: it is listed online already, or it beat recently enough that the
 *  offline it is wearing may simply not have caught up. */
function worthGuessingAt(device) {
  if (!neverAsked(device)) return false;
  return device.status === "online" || listingCouldBeLagging(device);
}

/** A machine nothing here has asked for yet: it has never answered and has
 *  never been blocked (no context either way), has no dial on it now, and is
 *  not barred. */
function neverAsked(device) {
  if (existingDeviceLifecycle(device.id)?.snapshot().securityStop || contextFor(device.id)) return false;
  return !connectionAttempts.isConnecting(device.id);
}

/**
 * A device for this call to open: online by the account list, with nothing
 * already working on it.
 *
 * A machine this client has already failed to reach is not also asked by the
 * presence cadence: its recovery supervisor owns the next attempt while it is
 * still online, and a reader Retry may bring that attempt forward.
 */
function wantsSession(device) {
  const context = contextFor(device.id);
  if (existingDeviceLifecycle(device.id)?.snapshot().securityStop) return false;
  if (connectionAttempts.isConnecting(device.id) || context?.blocked) return false;
  return device.status === "online" && !canAnswer(context);
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
