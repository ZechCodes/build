// One relay socket, end to end.
//
// The gateway token, `authenticate`, the device-key wait and the pin check,
// `session_init` / `session_accept`, the presence pushes, and the backoff
// reconnect that runs whether or not a DataChannel is carrying — one wire's
// whole life, hidden from the session riding it. Which socket generation is
// current is nobody else's question.
//
// A session's id and key are minted once per session, not once per socket
// (spec §SPA carrier and migration policy, 6): while something else is still
// carrying, a reconnecting socket re-presents the SAME session_init, which the
// bridge takes as a carrier re-attach. Re-keying under a live channel would
// strand every frame already in flight on it. A device that will not take the
// session back leaves it behind rather than presenting it forever.

import { openCarrier } from "./carrier.js";
import { relayInbox } from "./relayInbox.js";

const DEFAULT_OPEN_TIMEOUT_MS = 8000;
const DEFAULT_DEVICE_WAIT_MS = 8000;
const DEFAULT_ACCEPT_TIMEOUT_MS = 10000;

/** How long a lost socket waits before trying again, and how long it will ever
 *  wait. A relay that is down is down for everybody, so the wait grows. */
const FIRST_RETRY_MS = 400;
const LONGEST_RETRY_MS = 8000;

const noop = () => {};
const expiry = (ms, message) => new Promise((_, reject) => setTimeout(() => reject(new Error(message)), ms));

/**
 * @param preferDeviceId `() => device id or null`, re-read on every connect:
 *   this link is pinned to one machine, and a caller moving to another one is
 *   answered by the next socket, not by this one.
 * @param waitForDevice wait as long as it takes for a device to come online.
 *   This socket IS the "tell me when a device is back" channel, so a resume
 *   waits; a boot gives up after `deviceWaitMs` and shows the waiting screen.
 * @param carrying `() => whatever else is carrying this session`, asked at the
 *   moment a socket presents itself. Truthy is a carrier re-attach.
 * @param onConnecting a fresh socket is being opened.
 * @param onSession `({ sessionId, sessionKeyB64, deviceId })` — a session this
 *   link has just minted. Whatever was riding the one before it is not riding
 *   this one.
 * @param onRelay the relay slot: a `Carrier` while this socket carries, `null`
 *   the moment it does not. Awaited, so what re-establishes a session on a wire
 *   it just took finishes before the link calls itself up.
 */
// eslint-disable-next-line complexity -- ratchet: createRelayLink is at 13, cap 10 — reduce it, then drop this line
export function createRelayLink({
  relayUrl,
  transport,
  WebSocketImpl,
  fetchToken,
  getPinnedDeviceKey,
  preferDeviceId = () => null,
  waitForDevice = false,
  openTimeoutMs = DEFAULT_OPEN_TIMEOUT_MS,
  deviceWaitMs = DEFAULT_DEVICE_WAIT_MS,
  acceptTimeoutMs = DEFAULT_ACCEPT_TIMEOUT_MS,
  carrying = () => null,
  onDeviceKey = noop,
  onDeviceOffline = noop,
  onConnecting = noop,
  onSession = noop,
  onRelay = noop,
}) {
  // The relay is an untrusted broker: its device_key pushes are routing hints
  // only. Session keys are sealed exclusively to the api-pinned transport key
  // (bound into the device's Ed25519 registration), so a hostile relay cannot
  // substitute its own key and MITM the E2EE session.
  if (typeof getPinnedDeviceKey !== "function") {
    throw new Error("getPinnedDeviceKey is required — refusing to trust relay-supplied device keys");
  }

  let socket = null;
  let carrier = null;
  let generation = 0;
  let closed = false;
  let retryTimer = null;
  let retryIn = FIRST_RETRY_MS;
  // This session's id, key and device. They outlive every socket that carries
  // them; only a session nothing is riding is replaced.
  let session = null;

  /** The next message this handshake needs, or the reason there will not be
   *  one. Everything the predicate refuses goes past. */
  const waitFor = async (inbox, predicate, timeoutMs, timeoutMessage) => {
    const message = await inbox.matching(predicate, timeoutMs, timeoutMessage);
    if (!message) throw new Error("connection closed");
    return message;
  };

  /** This connection is over. Later generations are ignored, so one loss is one
   *  reconnect however many things noticed it. */
  const lost = (mine) => {
    if (closed || mine !== generation) return;
    generation += 1;
    carrier = null;
    onRelay(null);
    if (closed) return; // hearing the loss is what some callers close on
    const delay = retryIn;
    retryIn = Math.min(retryIn * 2, LONGEST_RETRY_MS);
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => {
      if (!closed) connect().catch(noop); // the failure already scheduled the next try
    }, delay);
  };

  // eslint-disable-next-line complexity -- ratchet: connect is at 15, cap 10 — reduce it, then drop this line
  async function connect() {
    const mine = ++generation;
    onConnecting();
    // Everything from here is one attempt: a failure before there is a socket
    // to close has no event to ride, so it says so through the same path.
    let ws = null;
    let reattaching = false;
    try {
      await transport.ready?.();
      // The api mints a short-lived gateway token for our logged-in session;
      // the relay validates it and routes us only to devices we own.
      const token = await fetchToken();
      ws = new WebSocketImpl(`${relayUrl}/ws/client`);
      socket = ws;
      // Every device_key / device_offline push (handshake or live) keeps the
      // caller's device store current, whichever device this session targets.
      const inbox = relayInbox(ws, (message) => {
        if (message.type === "device_key") onDeviceKey(message.device_id, message.transport_public_key);
        if (message.type === "device_offline") onDeviceOffline(message.device_id);
      });
      // Until there is a carrier, this socket's close is the only thing that
      // can report the loss; from then on the carrier reports it, exactly once.
      ws.addEventListener("close", () => {
        if (!carrier) lost(mine);
      });

      await Promise.race([
        new Promise((resolve, reject) => {
          ws.addEventListener("open", resolve);
          ws.addEventListener("error", reject);
        }),
        expiry(openTimeoutMs, "open timeout"),
      ]);
      ws.send(JSON.stringify({ type: "authenticate", token }));

      const wanted = preferDeviceId();
      const hello = await waitFor(
        inbox,
        (m) => m.type === "device_key" && (!wanted || m.device_id === wanted),
        waitForDevice ? 0 : deviceWaitMs,
        "no device online",
      );
      const deviceId = hello.device_id;
      // Seal to the api-pinned key, never the relay-pushed one; a mismatch means
      // the broker (or someone on the socket) is substituting keys — abort loudly.
      const pinnedKeyB64 = await getPinnedDeviceKey(deviceId);
      if (!pinnedKeyB64) {
        throw new Error(`no pinned transport key for device ${deviceId} — refusing to open a session`);
      }
      if (hello.transport_public_key !== pinnedKeyB64) {
        throw new Error("relay-supplied device key does not match the api-pinned key — possible tampering");
      }

      reattaching = Boolean(carrying() && session && session.deviceId === deviceId);
      const sessionId = reattaching ? session.sessionId : "sess-" + Math.random().toString(36).slice(2, 10);
      const init = await transport.createSessionInit({
        sessionId,
        deviceId,
        deviceTransportPublicKeyB64: pinnedKeyB64,
        sessionKeyB64: reattaching ? session.sessionKeyB64 : undefined,
      });
      ws.send(
        JSON.stringify({
          type: "session_init",
          session_id: sessionId,
          route_to: `device:${deviceId}`,
          session_init: init.sessionInit,
        }),
      );
      // Time-box session_accept and honor a device_offline for our target: a
      // device that dies right after its device_key snapshot would otherwise
      // hang this handshake forever.
      const answer = await waitFor(
        inbox,
        (m) => m.type === "session_accept" || (m.type === "device_offline" && m.device_id === deviceId),
        acceptTimeoutMs,
        "device did not accept the session",
      );
      if (answer.type === "device_offline") throw new Error("device went offline during the handshake");
      await transport.openSessionAccept({ sessionKeyB64: init.sessionKeyB64, envelope: answer.envelope });

      const reattached = reattaching;
      reattaching = false; // presented and taken: this session is the device's again
      session = { sessionId, sessionKeyB64: init.sessionKeyB64, deviceId };
      if (!reattached) onSession(session);
      carrier = openCarrier({ socket: ws, sessionId });
      carrier.onClose(() => lost(mine));
      await onRelay(carrier);
      retryIn = FIRST_RETRY_MS;
      // Our device dropping off the relay detaches this carrier. Whether that
      // ends the session is the switch's call: a peer path may still carry.
      inbox
        .matching((m) => m.type === "device_offline" && m.device_id === deviceId)
        .then((dropped) => dropped && lost(mine));
    } catch (error) {
      try {
        ws?.close();
      } catch {
        /* already gone */
      }
      // A re-attach the device would not take is not presented again: that
      // session is the device's to refuse, and the next socket mints a new one.
      if (reattaching) session = null;
      lost(mine);
      throw error;
    }
  }

  return {
    /** Bring this link up once. A failure is the caller's to hear — and a retry
     *  is already on its way, which a caller that wants none calls `close` on. */
    async start() {
      closed = false;
      await connect();
    },

    /** The device this link's session is with. */
    deviceId: () => session?.deviceId ?? null,

    /** Drop the socket and let the reconnect bring it back — how a caller that
     *  has moved to another machine gets `preferDeviceId` re-read. */
    dropSocket() {
      try {
        socket?.close();
      } catch {
        /* already gone */
      }
    },

    /** No more sockets: the client is done with this relay. */
    close() {
      closed = true;
      clearTimeout(retryTimer);
      carrier = null;
      try {
        socket?.close();
      } catch {
        /* already gone */
      }
    },
  };
}
