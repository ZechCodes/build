// How a browser finds one device, and nothing else.
//
// A rendezvous is the seam (spec rule 7): everything the SPA says to the relay
// is said through this interface, and a future direct-network mode — a LAN or
// Tailscale listener on the bridge — becomes a second implementation of it
// beside `createRelayRendezvous`, with no relay involved and nothing above this
// line changed.
//
//   { open(), mint({sessionId, sessionKeyB64}), signalCarrier(sessionId),
//     close(), isOpen(), onClosed(fn) }
//
// The relay half is one socket per device context (rule 5): it mints that
// device's app session and, when the terminals follow that device, its terminal
// session, and each minted session gets a signaling carrier on that one socket.
// It carries `session_init` / `session_accept` and `rtc.*` envelopes and NOTHING
// else — the browser is live over the DataChannels (rule 2), so the socket is
// open only while something is negotiating and closes again once nothing is
// (rule 4).
//
// There is therefore no reconnect loop here and no presence: a socket that goes
// rejects what was in flight on it and says so once. Whether that costs the
// device its context is the caller's call, not this module's.
//
// `relayInbox.js`'s one-waiter queue is not what reads this socket: several
// sessions negotiate on it at once, and a waiter that drops what its predicate
// refuses would eat another session's answer. `session_accept` is routed by the
// session id the relay puts on it, which is the same id it routes by.

import { openCarrier, sendOverSocket } from "./carrier.js";

/** How long the relay has to accept a socket, and the device to accept a
 *  session it has been offered. */
const DEFAULT_OPEN_TIMEOUT_MS = 8000;
const DEFAULT_ACCEPT_TIMEOUT_MS = 10000;

const newSessionId = () => "sess-" + Math.random().toString(36).slice(2, 10);

/**
 * @param deviceId the one machine this rendezvous is with. A relay socket is
 *   per device context, so nothing here asks who answered.
 * @param getPinnedDeviceKey the api-pinned transport key for a device. The
 *   relay is an untrusted broker: session keys are sealed exclusively to this
 *   key (bound into the device's Ed25519 registration), and the relay is never
 *   asked what a device's key is.
 */
export function createRelayRendezvous({
  deviceId,
  relayUrl,
  transport,
  WebSocketImpl,
  fetchToken,
  getPinnedDeviceKey,
  openTimeoutMs = DEFAULT_OPEN_TIMEOUT_MS,
  acceptTimeoutMs = DEFAULT_ACCEPT_TIMEOUT_MS,
}) {
  if (typeof getPinnedDeviceKey !== "function") {
    throw new Error("getPinnedDeviceKey is required — refusing to trust relay-supplied device keys");
  }

  let socket = null;
  let opening = null;
  // Which dial this rendezvous is on. Closing it — or the socket going — moves
  // it on, so a dial still in flight knows the answer it is about to give is
  // nobody's: it opens no socket, and a socket it did open is shut. The relay
  // is held open only while something is negotiating (rule 4), and an
  // authenticated socket nothing owns would be held open by nothing at all.
  let era = 0;
  const carriers = new Set();
  const closedListeners = new Set();
  // sessionId → what the mint waiting for that device's answer is settled with.
  const awaiting = new Map();

  /** This socket is over. Every carrier on it ends, every mint waiting on it
   *  fails, and — when nobody asked for it — the caller hears about it once. */
  const dropped = (ws, { deliberate = false } = {}) => {
    if (socket !== ws) return; // a socket this rendezvous has already replaced
    socket = null;
    opening = null;
    era += 1;
    for (const carrier of [...carriers]) carrier.close();
    carriers.clear();
    for (const settle of [...awaiting.values()]) settle.fail(new Error("the rendezvous closed"));
    awaiting.clear();
    if (!deliberate) for (const listener of [...closedListeners]) listener();
  };

  /** One session's answer, to the mint that is waiting for it. The relay routes
   *  by session id and puts it on what it forwards, so this reads the same
   *  field the relay read. */
  const route = (message) => {
    if (message.type !== "session_accept") return;
    awaiting.get(message.session_id)?.answer(message);
  };

  async function dial(dialledIn) {
    await transport.ready?.();
    // The api mints a short-lived gateway token for our logged-in session; the
    // relay validates it and routes us only to devices we own.
    const token = await fetchToken();
    // Called off while that token was in flight: the socket this would have
    // opened has no owner, so it is not opened at all.
    if (dialledIn !== era) throw new Error("the rendezvous closed");
    const ws = new WebSocketImpl(`${relayUrl}/ws/client`);
    socket = ws;
    ws.addEventListener("message", (event) => {
      try {
        route(JSON.parse(typeof event.data === "string" ? event.data : event.data.toString()));
      } catch {
        /* the relay speaks JSON; anything else is not a message we have */
      }
    });
    ws.addEventListener("close", () => dropped(ws));
    try {
      await openedWithin(ws, openTimeoutMs);
      if (dialledIn !== era) throw new Error("the rendezvous closed"); // shut below, with nothing sent on it
      sendOverSocket(ws, JSON.stringify({ type: "authenticate", token }));
    } catch (error) {
      try {
        ws.close();
      } catch {
        /* already gone */
      }
      dropped(ws, { deliberate: true }); // the caller is being told; nobody else needs to be
      throw error;
    }
    return ws;
  }

  /** The socket, opened if it is not there. A second caller joins the attempt
   *  in flight rather than starting another. */
  const open = () => (opening ||= startDial());

  function startDial() {
    // Only this attempt's own failure clears the slot: one that was called off
    // must not throw away the dial that replaced it.
    const attempt = dial(era).catch((error) => {
      if (opening === attempt) opening = null;
      throw error;
    });
    return attempt;
  }

  /**
   * The device's answer to one `session_init`, or the reason there is none.
   *
   * One waiter per session id, because that is all the relay's routing can
   * answer: it forwards the accept under the id it routed by, and a second
   * waiter under the same id could only take the first one's place — whose
   * deadline would then delete it, and whose device's answer would reach
   * nobody. Overlapping mints of one session are refused here, in the words
   * that say why, rather than silently losing one of them.
   */
  const accepted = (sessionId) => {
    if (awaiting.has(sessionId)) throw new Error(`session ${sessionId} is already being minted on this rendezvous`);
    return new Promise((resolve, reject) => {
      const deadline = setTimeout(() => {
        awaiting.delete(sessionId);
        reject(new Error("device did not answer"));
      }, acceptTimeoutMs);
      const settle = (finish) => (value) => {
        clearTimeout(deadline);
        awaiting.delete(sessionId);
        finish(value);
      };
      awaiting.set(sessionId, { answer: settle(resolve), fail: settle(reject) });
    });
  };

  /**
   * One session with this device, opened over the rendezvous.
   *
   * A session's id and key are minted once per session, not once per socket: a
   * caller re-attaching a session it already holds passes both back, and the
   * bridge takes the same init as a carrier re-attach rather than a new
   * session. Nothing waits for the relay to announce the device first — the
   * relay no longer says (rule 6), and the key is the api's either way.
   */
  async function mint({ sessionId, sessionKeyB64 } = {}) {
    const ws = await open();
    const pinnedKeyB64 = await getPinnedDeviceKey(deviceId);
    if (!pinnedKeyB64) {
      throw Object.assign(new Error(`no pinned transport key for device ${deviceId} — refusing to open a session`), {
        securityCritical: true,
      });
    }
    const id = sessionId || newSessionId();
    const init = await transport.createSessionInit({
      sessionId: id,
      deviceId,
      deviceTransportPublicKeyB64: pinnedKeyB64,
      sessionKeyB64,
    });
    const answer = accepted(id);
    answer.catch(() => {}); // awaited below; a send that throws first must not leave it unhandled
    sendOverSocket(
      ws,
      JSON.stringify({ type: "session_init", session_id: id, route_to: `device:${deviceId}`, session_init: init.sessionInit }),
    );
    await transport.openSessionAccept({ sessionKeyB64: init.sessionKeyB64, envelope: (await answer).envelope });
    return { sessionId: id, sessionKeyB64: init.sessionKeyB64, deviceId };
  }

  return {
    open,
    mint,

    /** The wire one session's `rtc.*` rides while this rendezvous is open. It
     *  is a lease on the socket: letting it go leaves the socket to the other
     *  sessions, and the rendezvous closing ends it. */
    signalCarrier(sessionId) {
      if (!socket) throw new Error("the rendezvous is not open");
      const carrier = openCarrier({ socket, sessionId });
      carriers.add(carrier);
      carrier.onClose(() => carriers.delete(carrier));
      return carrier;
    },

    isOpen: () => Boolean(socket),

    /** A socket that went without being asked to. The caller decides what that
     *  costs — there is no reconnect here. Returns the unsubscribe. */
    onClosed(fn) {
      closedListeners.add(fn);
      return () => closedListeners.delete(fn);
    },

    /** Nothing is negotiating: the relay is not held open between upgrades. */
    close() {
      era += 1; // whatever is being dialled is nobody's now
      opening = null;
      const ws = socket;
      if (!ws) return;
      dropped(ws, { deliberate: true });
      try {
        ws.close();
      } catch {
        /* already gone */
      }
    },
  };
}

/** The socket comes up, or it does not within the window. However it settles,
 *  the deadline stops ticking: a timer left armed on a socket that is already
 *  up fires into a suite that has moved on. */
function openedWithin(ws, timeoutMs) {
  let deadline;
  const up = new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve);
    ws.addEventListener("error", reject);
  });
  const expiry = new Promise((_, reject) => {
    deadline = setTimeout(() => reject(new Error("the relay did not answer")), timeoutMs);
  });
  return Promise.race([up, expiry]).finally(() => clearTimeout(deadline));
}
