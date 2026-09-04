// The E2EE relay session core — runtime-agnostic (injected WebSocket + transport,
// so vitest drives it with fakes and the browser with the real thing).
//
// Contract (browser ↔ relay /ws/client):
//   1. first frame {type:"authenticate", token} — token minted by POST /api/gateway-token
//   2. relay replies {type:"authenticated"}, then pushes {type:"device_key",
//      device_id, transport_public_key} for each of the user's online devices
//      (and again whenever one comes online later)
//   3. we seal a fresh session key to the chosen device (session_init), the device
//      answers session_accept, and from then on frames are opaque e2ee_envelopes
//   4. {type:"device_offline", device_id} tells us a device dropped
//
// One object per live session. RPC replies resolve through a pending map with
// timeouts so calls fail fast instead of hanging on a dead session.
//
// The socket the handshake ran on is this session's FIRST carrier, not its only
// one: `peer(carrier)` hands it a DataChannel to ride instead, and the session
// ends when its last carrier is gone (see sessionSwitch.js). The key, the
// frames and the pending map are the same either way — a carrier is a wire.

import { openCarrier } from "./carrier.js";
import { createSessionSwitch } from "./sessionSwitch.js";

const DEFAULT_DEVICE_WAIT_MS = 8000;
const DEFAULT_RPC_TIMEOUT_MS = 12000;
const DEFAULT_ACCEPT_TIMEOUT_MS = 10000;

export async function openRelaySession({
  relayUrl,
  transport,
  WebSocketImpl,
  fetchToken,
  getPinnedDeviceKey,
  preferDeviceId = null,
  waitForDevice = false,
  deviceWaitMs = DEFAULT_DEVICE_WAIT_MS,
  acceptTimeoutMs = DEFAULT_ACCEPT_TIMEOUT_MS,
  isPaused = () => false,
  onDeviceKey = () => {},
  onDeviceOffline = () => {},
  onLost = () => {},
  onPush = () => {},
}) {
  // The relay is an untrusted broker: its device_key pushes are routing hints
  // only. Session keys are sealed exclusively to the api-pinned transport key
  // (bound into the device's Ed25519 registration), so a hostile relay cannot
  // substitute its own key and MITM the E2EE session.
  if (typeof getPinnedDeviceKey !== "function") {
    throw new Error("getPinnedDeviceKey is required — refusing to trust relay-supplied device keys");
  }
  await transport.ready?.();
  // The api mints a short-lived gateway token for our logged-in session; the
  // relay validates it and routes us only to devices we own.
  const token = await fetchToken();
  const ws = new WebSocketImpl(`${relayUrl}/ws/client`);

  const queue = [];
  const waiters = [];
  let live = false;
  let lost = false;
  let sessionKey = null;
  let deviceId = null;
  const pending = new Map();
  let requestId = 0;

  const severSession = () => {
    if (lost) return;
    lost = true;
    for (const { reject } of pending.values()) reject(new Error("your device went offline"));
    pending.clear();
    onLost();
  };

  /** One decrypted frame off whichever carrier brought it: somebody's answer,
   *  or the bridge telling us something moved. */
  const takeEnvelope = async (envelope) => {
    let frame;
    try {
      frame = await transport.decryptEnvelope({ sessionKeyB64: sessionKey, envelope });
    } catch {
      return;
    }
    const payload = frame.payload;
    const pend = payload && payload.id !== undefined ? pending.get(payload.id) : null;
    if (pend) {
      pending.delete(payload.id);
      payload.ok ? pend.resolve(payload.result) : pend.reject(new Error(payload.error));
      return;
    }
    // Nobody asked for this: the bridge is telling us something moved. A frame
    // with no request behind it and a `type` is a push (board.changed,
    // entity.changed); anything else is a reply to a call that already timed
    // out, and has nowhere left to go.
    if (payload && payload.type) onPush(payload);
  };

  const deliver = (message) => (waiters.length ? waiters.shift()(message) : queue.push(message));
  ws.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    // Every device_key / device_offline push (handshake or live) keeps the
    // caller's device store current, whichever device the session targets.
    if (message.type === "device_key") onDeviceKey(message.device_id, message.transport_public_key);
    if (message.type === "device_offline") onDeviceOffline(message.device_id);
    if (!live) return deliver(message); // handshake phase: recvType drains the queue
    // Our device dropping off the relay detaches the relay carrier. Whether
    // that ends the session is the switch's call: a peer path may still carry.
    if (message.type === "device_offline" && message.device_id === deviceId) carrierSwitch.relay(null);
  });
  ws.addEventListener("close", () => {
    if (!live) deliver({ type: "__closed" }); // fail the handshake cleanly
  });

  const recv = () => new Promise((resolve) => (queue.length ? resolve(queue.shift()) : waiters.push(resolve)));
  const recvMatching = async (predicate) => {
    for (;;) {
      const message = await recv();
      if (message.type === "__closed") throw new Error("connection closed");
      if (predicate(message)) return message;
    }
  };
  const send = (obj) => ws.send(JSON.stringify(obj));

  let sessionId;
  let sessionKeyB64;
  try {
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", reject);
    });
    send({ type: "authenticate", token });

    // Wait for the target device's key. At boot we give up after deviceWaitMs (the
    // gate shows the waiting screen); during a resume we wait as long as it takes —
    // this socket IS the "tell me when a device comes back" channel.
    const wantedKey = (m) =>
      m.type === "device_key" && (!preferDeviceId || m.device_id === preferDeviceId);
    const hello = waitForDevice
      ? await recvMatching(wantedKey)
      : await Promise.race([
          recvMatching(wantedKey),
          new Promise((_, reject) => setTimeout(() => reject(new Error("no device online")), deviceWaitMs)),
        ]);
    deviceId = hello.device_id;

    // Seal to the api-pinned key, never the relay-pushed one; a mismatch means
    // the broker (or someone on the socket) is substituting keys — abort loudly.
    const pinnedKeyB64 = await getPinnedDeviceKey(deviceId);
    if (!pinnedKeyB64) throw new Error(`no pinned transport key for device ${deviceId} — refusing to open a session`);
    if (hello.transport_public_key !== pinnedKeyB64) {
      throw new Error("relay-supplied device key does not match the api-pinned key — possible tampering");
    }

    sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
    const init = await transport.createSessionInit({
      sessionId,
      deviceId,
      deviceTransportPublicKeyB64: pinnedKeyB64,
    });
    sessionKeyB64 = init.sessionKeyB64;
    send({ type: "session_init", session_id: sessionId, route_to: `device:${deviceId}`, session_init: init.sessionInit });
    // Time-box session_accept and honor a device_offline for our target: a
    // device that dies right after its device_key snapshot would otherwise hang
    // this handshake forever (latching the caller's connect/resume flags).
    const acceptOrOffline = await Promise.race([
      recvMatching(
        (m) => m.type === "session_accept" || (m.type === "device_offline" && m.device_id === deviceId),
      ),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error("device did not accept the session")), acceptTimeoutMs),
      ),
    ]);
    if (acceptOrOffline.type === "device_offline") throw new Error("device went offline during the handshake");
    await transport.openSessionAccept({ sessionKeyB64, envelope: acceptOrOffline.envelope });
  } catch (error) {
    lost = true; // a failed handshake never fires onLost — the caller retries
    try {
      ws.close();
    } catch {
      /* already gone */
    }
    throw error;
  }
  sessionKey = sessionKeyB64;
  live = true;

  let carrier = null;
  let onCarrierChange = () => {};
  const carrierSwitch = createSessionSwitch({
    session: {
      rideOn: (taken) => {
        carrier = taken;
        taken?.onEnvelope(takeEnvelope);
      },
    },
    onActive: () => onCarrierChange(),
    onIdle: severSession,
  });
  const relayCarrier = openCarrier({ socket: ws, sessionId });
  relayCarrier.onClose(() => carrierSwitch.relay(null));
  carrierSwitch.relay(relayCarrier);

  /** One encrypted frame out over `wire`, and the reply it is waiting for. */
  async function request(wire, method, params, timeoutMs) {
    if (isPaused()) throw new Error("your device is offline — reconnecting…");
    if (lost || !wire) throw new Error("your device went offline");
    const rid = "r" + ++requestId;
    const envelope = await transport.encryptFrame({
      sessionKeyB64,
      outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
      frameFields: { frame_type: "data", sender: "client", payload: { method, id: rid, params } },
    });
    const reply = new Promise((resolve, reject) => pending.set(rid, { resolve, reject }));
    wire.send(envelope);
    return Promise.race([
      reply,
      new Promise((_, reject) =>
        setTimeout(() => {
          pending.delete(rid);
          reject(new Error(`${method} timed out`));
        }, timeoutMs),
      ),
    ]);
  }

  /** Sever this session deliberately (e.g. switching devices) — no onLost. */
  function close() {
    carrierSwitch.close();
    lost = true;
    for (const { reject } of pending.values()) reject(new Error("session closed"));
    pending.clear();
    try {
      ws.close();
    } catch {
      /* already gone */
    }
  }

  return {
    deviceId,
    call: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) => request(carrier, method, params, timeoutMs),
    /** Signaling is pinned to the relay carrier: `rtc.*` never rides the
     *  channel it negotiates, so an ICE restart works while the channels are
     *  down (spec §Signaling). */
    signal: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) =>
      request(carrierSwitch.relayCarrier(), method, params, timeoutMs),
    /** Ride this DataChannel instead of the relay, or `null` to fall back. */
    peer: (peerCarrier) => carrierSwitch.peer(peerCarrier),
    /** What re-establishes this session on a carrier it has just taken —
     *  `session.hello` and a read of every mounted surface. Registered after
     *  the session is handed over, so the first relay attach is the caller's
     *  own greeting, not a second one. */
    onCarrier: (fn) => (onCarrierChange = fn),
    close,
  };
}
