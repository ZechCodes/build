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
// its calls are refused in those words, and a Retry or the presence poll runs
// the sequence again. Nothing retries on a loop.
//
// The account-wide waiting screen is for the state where nothing at all can
// answer, which is the only one the user can do anything about.

import * as transport from "@build/secure-transport";
import { RELAY_URL } from "./config.js";
import { createRelayRendezvous } from "./core/rendezvous.js";
import { openSession } from "./core/session.js";
import { openPeerLink } from "./core/peerLink.js";
import { connectionDiagnosticHistory } from "./core/connectionDiagnostics.js";
import { isSignaling } from "./core/sessionSwitch.js";
import { fetchGatewayToken, fetchIceServers } from "./api.js";
import { App, rememberSelectedDevice } from "./app.js";
import {
  adoptBridgeSelection,
  adoptDeviceSession,
  canAnswer,
  closeQuietly,
  contextFor,
  homeContext,
  knownContexts,
  knownDeviceContext,
  liveContexts,
  onDeviceStateChanged,
  releaseGreeting,
  retireDeviceContext,
  setContextOffline,
} from "./core/deviceContexts.js";
import { blockedMark, deviceAwayMark, deviceAwayText } from "./core/deviceAway.js";
import { deviceNameOf } from "./core/devicePolicy.js";
import { pinnedDeviceTransportKey } from "./devices.js";
import { followTerminalDevice, provideTerminalSessions, terminalDeviceId } from "./terminal/manager.js";
import { flushCaptures } from "./core/composeView.js";
import { dispatchChangeEvent, greetBridge } from "./core/changeEvents.js";
import { deliverFeed, joinFeed } from "./core/taskFeed.js";

// ---- one rendezvous per device (spec rules 4, 5 and 7) -----------------------

// How each machine is found, one rendezvous per device context. The map owns
// them because a machine can be dialled before it has a context — a connect
// that fails never lands one — and the context carries the same object, so
// anything holding a device can find the way to it.
const rendezvousByDevice = new Map();

/** The way to one machine, opened on demand by whatever needs it. A relay
 *  socket today; a direct-network listener is a second implementation of the
 *  same interface, and nothing below this line would know (rule 7). */
function rendezvousFor(deviceId) {
  const known = rendezvousByDevice.get(deviceId);
  if (known) return known;
  const rendezvous = createRelayRendezvous({
    deviceId,
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    // The api-pinned transport key, never a relay-supplied one: the relay is an
    // untrusted broker and session keys are sealed exclusively to that key.
    getPinnedDeviceKey: pinnedDeviceTransportKey,
  });
  rendezvousByDevice.set(deviceId, rendezvous);
  return rendezvous;
}

// How many things on one machine need its rendezvous open right now: a connect
// sequence negotiating, an ICE restart asking for one, a terminal session being
// minted. The relay is not held between them (rule 4), so the last to let go
// closes it.
const negotiating = new Map(); // deviceId → how many
const rendezvousEra = new Map(); // deviceId → forced-close generation

/** Ask for this machine's rendezvous and say it is wanted open. */
function holdRendezvous(deviceId) {
  negotiating.set(deviceId, (negotiating.get(deviceId) || 0) + 1);
  return rendezvousFor(deviceId);
}

/** Done negotiating. Nothing else waiting on it closes the socket. */
function releaseRendezvous(deviceId, expectedEra = null) {
  if (expectedEra !== null && (rendezvousEra.get(deviceId) || 0) !== expectedEra) return;
  const left = (negotiating.get(deviceId) || 1) - 1;
  if (left > 0) {
    negotiating.set(deviceId, left);
    return;
  }
  closeRendezvous(deviceId);
}

/** Close this machine's rendezvous whoever was holding it: the device is
 *  blocked, or gone, and nothing is negotiating with it any more. */
function closeRendezvous(deviceId) {
  rendezvousEra.set(deviceId, (rendezvousEra.get(deviceId) || 0) + 1);
  negotiating.delete(deviceId);
  rendezvousByDevice.get(deviceId)?.close();
}

/** Let go of every rendezvous (sign-out, teardown): they are this account's,
 *  and a socket left open is a socket for an account that has gone. */
export function forgetRendezvousSockets() {
  for (const deviceId of [...rendezvousByDevice.keys()]) closeRendezvous(deviceId);
  rendezvousByDevice.clear();
  negotiating.clear();
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
async function mintAppSession(deviceId) {
  try {
    return await openSession({
      rendezvous: rendezvousFor(deviceId),
      transport,
      deviceId,
      // One device's offline state pauses one device's calls.
      isPaused: () => contextFor(deviceId)?.offline === true,
      // The only carrier this session ever had has gone (rule 3's `lost`).
      onLost: () => goOffline(deviceId),
      // The bridge saying something moved. A frame nobody asked for reaches the
      // surfaces showing that device's state — except the signaling pushes,
      // which belong to the upgrade negotiating them.
      onPush: (payload) => {
        if (!isSignaling(payload.type)) dispatchChangeEvent(payload, deviceId);
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
function openDirectLink(deviceId, session) {
  return openPeerLink({
    signal: (method, params) => failingAs("refused", session.call(method, params)),
    fetchIceServers: () => failingAs("ice-servers", fetchIceServers()),
    onPush: session.onPush,
    diagnosticId: `${deviceId}:${session.sessionId}`,
    onConnected: () => releaseRendezvous(deviceId),
    onFailed: async () => {
      holdRendezvous(deviceId);
      await session.reattachSignaling();
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
async function connectOverChannels(deviceId, stillWanted = () => true) {
  if (!globalThis.RTCPeerConnection) {
    throw becauseOf(new Error("this browser cannot open a direct connection"), "no-webrtc");
  }
  holdRendezvous(deviceId); // until the channels are open, or the device is blocked
  const session = await mintAppSession(deviceId);
  let link;
  try {
    link = await openDirectLink(deviceId, session);
  } catch (error) {
    closeQuietly(session); // nothing ever carried it
    throw error;
  }
  if (!stillWanted()) {
    // Closed before the link is dropped, so the session reports no loss on the
    // way out: this machine is not offline, it is one this dial no longer has.
    closeQuietly(session);
    link.close();
    throw new Error(`device ${deviceId} is no longer on this account`);
  }
  try {
    return await landSession(session, link);
  } catch (error) {
    closeQuietly(session);
    link.close();
    throw error;
  }
}

// The machines this layer has a connect in flight at, and the attempt itself: a
// second caller — the presence poll, a reader pressing Retry — joins the one in
// flight rather than opening a second session at the same machine.
const dialling = new Map(); // deviceId → the connect in flight

// Which dial at one machine is the one this layer is still waiting on. Retiring
// a device moves its count on, so a connect in flight at a machine the account
// has let go of lands nothing and blocks nothing: whatever it says is about a
// machine nobody has.
const dialEra = new Map(); // deviceId → which dial

/** Whether the account still has the machine this dial was started for. */
const stillAsking = (deviceId, era) => dialEra.get(deviceId) === era;

/** Call off whatever is being dialled at this machine. */
function stopDialling(deviceId) {
  dialEra.set(deviceId, (dialEra.get(deviceId) || 0) + 1);
  dialling.delete(deviceId);
}

/**
 * Connect one device and land it (spec rule 3's sequence).
 *
 * Any failure blocks that device: its rendezvous and session close, its link is
 * dropped, its rows grey with the reason, and whatever was queued for a wire is
 * refused in those words. Nothing falls back to the relay and nothing retries
 * on a loop — the Retry control on its strip and on the waiting screen calls
 * this, and so does the presence poll when the machine comes back.
 */
export function connectDevice(deviceId) {
  handTerminalsTheirMint();
  const inFlight = dialling.get(deviceId);
  if (inFlight) return inFlight;
  const era = (dialEra.get(deviceId) || 0) + 1;
  dialEra.set(deviceId, era);
  const attempt = connectOnce(deviceId, era).finally(() => {
    if (dialling.get(deviceId) === attempt) dialling.delete(deviceId);
  });
  dialling.set(deviceId, attempt);
  return attempt;
}

async function connectOnce(deviceId, era) {
  // Barred for good: offering the same pinned key to the same impostor again
  // would neither fix that nor tell anyone about it.
  if (securityStops.has(deviceId)) throw new Error(securityStops.get(deviceId));
  try {
    // It may answer after the account has let it go, and the machine it answers
    // for is not the machine this dial was started for any more: nothing it
    // opened is landed, and nothing that outlived it is touched.
    return await connectOverChannels(deviceId, () => stillAsking(deviceId, era));
  } catch (error) {
    // A machine the account has let go of is not blocked: blocking it would put
    // it back in the rail, greyed, with a Retry that dials a device nobody has.
    if (stillAsking(deviceId, era)) {
      barredBySecurity(deviceId, error);
      blockDevice(deviceId, reasonOf(error));
    }
    throw error;
  }
}

/** Put this device's streams on the connection that just opened. The two
 *  channels are one connection: whichever goes first takes the other, and the
 *  session hears that as its carrier going, which is rule 3's `lost`. */
function holdPeerLink(context, link) {
  context.peerLink = link;
  for (const carrier of [link.app, link.term]) {
    carrier.onClose(() => {
      if (context.peerLink === link) dropPeerLink(context);
    });
  }
}

/** Idempotent, and the single point where both streams are handed back at once:
 *  nothing may go on riding a peer connection that is going away. */
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
function followTerminalsIfTheirs(context, options) {
  if (context.deviceId === terminalDeviceId()) followTerminalDevice(options);
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
  const rendezvous = holdRendezvous(deviceId);
  const era = rendezvousEra.get(deviceId) || 0;
  try {
    const minted = await rendezvous.mint({});
    let released = false;
    return {
      ...minted,
      release: () => {
        if (released) return;
        released = true;
        releaseRendezvous(deviceId, era);
      },
    };
  } catch (error) {
    releaseRendezvous(deviceId, era);
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
  standDown(deviceId, blockedMark(reason));
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
  if (dialling.has(deviceId)) return;
  standDown(deviceId, {});
}

/** One machine stops answering, however it stopped. The mark says why, and the
 *  same mark is what everything waiting on that machine is refused with. */
function standDown(deviceId, mark) {
  const context = knownDeviceContext(deviceId);
  setContextOffline(deviceId, mark);
  // Closed before the link is dropped: a session told it lost its carrier would
  // report this same machine lost, through this same path, all over again.
  context.session?.fail?.(new Error(deviceAwayMark(context)));
  closeQuietly(context.session);
  dropPeerLink(context);
  closeRendezvous(deviceId);
  // Home may have moved off it — and if it has not, the surfaces that follow
  // home still have to say the device they are about cannot be reached.
  syncHome(context);
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
  const context = contextFor(deviceId);
  // A dial in flight at this machine is called off first: whatever it lands or
  // fails at is about a machine the account no longer has.
  stopDialling(deviceId);
  // Closed first: this machine is not lost, it is gone, and nothing is to be
  // blocked on the way out.
  closeQuietly(context?.session);
  dropPeerLink(context);
  closeRendezvous(deviceId);
  rendezvousByDevice.delete(deviceId);
  securityStops.delete(deviceId);
  return retireDeviceContext(deviceId);
}

// A machine this client will not dial again for the life of the tab: the key
// offered for it was not the key this account pinned, so whatever answered is
// not the machine that was paired. What it said is kept for the screen that has
// room to say it.
const securityStops = new Map(); // deviceId → what the refusal said

/** Whether this failure is a stop rather than an outage — and if it is, the
 *  machine is barred here, once, wherever the error was caught. */
function barredBySecurity(deviceId, error) {
  if (!error?.securityCritical) return false;
  securityStops.set(deviceId, error.message);
  return true;
}

/** What to say about a machine this client has stopped dialling, or "" while
 *  every machine is merely unreachable. A stop is the one connection failure a
 *  reader can act on and the only one that never resolves itself, so the
 *  screen holding the app says it out loud. */
export const securityStopText = () => [...securityStops.values()][0] || "";

/** Let go of every bar (sign-out, teardown): they are this account's, and the
 *  next account's machines have not been refused anything. */
export function forgetSecurityStops() {
  securityStops.clear();
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
export function greetLiveBridge(context, { suppressFailure = true } = {}) {
  const session = context?.session;
  if (!session) return Promise.resolve(false);
  const repository = context.chatRepository;
  const greeting = greetBridge(session.call, {
    strict: !suppressFailure,
    deviceId: session.deviceId,
    // The device's context may have been retargeted onto a newer session while
    // this greeting was in flight; that greeting belongs to the session that
    // asked for it, not to the one the device is on now.
    isCurrent: () => contextFor(session.deviceId)?.session === session,
    onGreeting: (greeting) => repository?.configureCapabilities(greeting),
    // The session first, so a gate that lets the app back in finds it there;
    // then the device's context, which is where every surface reads what this
    // machine's bridge speaks (core/deviceContexts.js).
    install: (selection) => {
      const adapter = session.installAdapter(selection);
      adoptBridgeSelection(context, selection, adapter);
      return adapter;
    },
  }).catch((error) => {
    // Wake greeting-dependent reads only after this failed session is marked
    // unavailable; otherwise the feed can send a request between the rejected
    // hello and the outer connection attempt's cleanup.
    if (!suppressFailure && contextFor(session.deviceId)?.session === session) {
      setContextOffline(session.deviceId, blockedMark(reasonOf(error)));
    }
    throw error;
  }).finally(() => {
      // Only this session's own: a newer one has armed a greeting of its own,
      // and what this one failed to say is no answer about that bridge.
      if (context.session === session) releaseGreeting(context);
    });
  return suppressFailure ? greeting.catch(() => false) : greeting;
}

/** Everything a device gets the moment it is live over its channels: the
 *  registry adopts it (a reconnect keeps that device's scope, drafts and
 *  controllers and only replaces its transport), it holds its connection, the
 *  bridge is greeted over that connection, and the feed starts reading it.
 *
 *  Nothing is dispatched to the user's surfaces before both channels are open
 *  (rule 2), so this is the first moment anything is asked of the machine. */
async function landSession(session, link) {
  const previous = contextFor(session.deviceId);
  // Closed before its link is dropped: a session told it lost its carrier would
  // block the very device that is landing.
  if (previous?.session && previous.session !== session) closeQuietly(previous.session);
  dropPeerLink(previous); // it was carrying the session this one replaces
  const context = adoptDeviceSession(session);
  holdPeerLink(context, link);
  // Every later carrier change re-establishes the session on the wire it took:
  // session.hello, and a read of every mounted surface.
  let initialGreeting = true;
  session.onCarrier(() => greetLiveBridge(context, { suppressFailure: !initialGreeting }));
  // Bind the app channel before releasing signaling. The carrier callback is
  // the acknowledged session.hello, so the relay cannot disappear in the gap
  // between WebRTC opening and the application session becoming usable.
  try {
    await session.peer(link.app);
  } finally {
    initialGreeting = false;
  }
  if (contextFor(session.deviceId)?.session !== session) throw new Error("session replaced during greeting");
  releaseRendezvous(session.deviceId);
  // A device the feed is not polling yet — the account's first session, one a
  // late device just opened — gets its own board watcher and reads it as soon
  // as its greeting is in.
  joinFeed(context);
  syncHome(context);
  followTerminalsIfTheirs(context, { freshSession: true });
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
  const unblocked = retry ? askBlockedMachinesAgain() : [];
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
    if (!context.blocked || securityStops.has(context.deviceId)) continue;
    setContextOffline(context.deviceId, { sinceMs: context.offlineSince, blocked: null });
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
 */
function guessAtStaleDevices() {
  if (liveContexts().length) return []; // something is answering; the poll will hear the rest
  return App.devices.filter(neverAsked).map((device) => connectDevice(device.id));
}

/** A machine nothing here has asked for yet: it has never answered and has
 *  never been blocked (no context either way), has no dial on it now, and is
 *  not barred. */
function neverAsked(device) {
  if (securityStops.has(device.id) || contextFor(device.id)) return false;
  return !dialling.has(device.id);
}

/**
 * A device for this call to open: online by the account list, with nothing
 * already working on it.
 *
 * A machine this client has already failed to reach is not asked again on a
 * cadence (rule 3): it wears its reason until a reader retries it, or until the
 * api says it went and came back — which clears the block on the way past.
 */
function wantsSession(device) {
  const context = contextFor(device.id);
  if (securityStops.has(device.id)) return false; // barred: retrying offers the same key to the same impostor
  if (dialling.has(device.id) || context?.blocked) return false;
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
