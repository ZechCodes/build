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
// One object per live session. What the session then IS — its key, its frames,
// its pending calls and its demux — is core/sessionRpc.js, the same machinery
// the terminal session runs on; what is here is the relay handshake that mints
// it and the interface the app calls it through.
//
// The socket the handshake ran on is this session's FIRST carrier, not its only
// one: `peer(carrier)` hands it a DataChannel to ride instead, and the session
// ends when its last carrier is gone (see sessionSwitch.js). Everything above
// the wire is the same either way — a carrier is a wire.

import { openCarrier } from "./carrier.js";
import { relayInbox } from "./relayInbox.js";
import { createSessionRpc } from "./sessionRpc.js";
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

  let deviceId = null;

  // Every device_key / device_offline push (handshake or live) keeps the
  // caller's device store current, whichever device the session targets.
  const inbox = relayInbox(ws, (message) => {
    if (message.type === "device_key") onDeviceKey(message.device_id, message.transport_public_key);
    if (message.type === "device_offline") onDeviceOffline(message.device_id);
  });
  const recvMatching = async (predicate, timeoutMs, timeoutMessage) => {
    const message = await inbox.matching(predicate, timeoutMs, timeoutMessage);
    if (!message) throw new Error("connection closed");
    return message;
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
    const hello = await recvMatching(wantedKey, waitForDevice ? 0 : deviceWaitMs, "no device online");
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
    const acceptOrOffline = await recvMatching(
      (m) => m.type === "session_accept" || (m.type === "device_offline" && m.device_id === deviceId),
      acceptTimeoutMs,
      "device did not accept the session",
    );
    if (acceptOrOffline.type === "device_offline") throw new Error("device went offline during the handshake");
    await transport.openSessionAccept({ sessionKeyB64, envelope: acceptOrOffline.envelope });
  } catch (error) {
    // A failed handshake never fires onLost — the caller retries.
    try {
      ws.close();
    } catch {
      /* already gone */
    }
    throw error;
  }
  const rpc = createSessionRpc({
    transport,
    sessionId,
    sessionKeyB64,
    deviceId,
    noCarrier: () => new Error("your device went offline"),
  });
  rpc.onPush(onPush);

  let severed = false;
  /** Nothing is carrying this session any more. The caller hears it once. */
  const severSession = () => {
    if (severed) return;
    severed = true;
    rpc.fail(new Error("your device went offline"));
    onLost();
  };

  let onCarrierChange = () => {};
  const carrierSwitch = createSessionSwitch({
    session: rpc,
    onActive: () => onCarrierChange(),
    onIdle: severSession,
  });
  const relayCarrier = openCarrier({ socket: ws, sessionId });
  relayCarrier.onClose(() => carrierSwitch.relay(null));
  carrierSwitch.relay(relayCarrier);
  // Our device dropping off the relay detaches the relay carrier. Whether that
  // ends the session is the switch's call: a peer path may still carry.
  inbox
    .matching((m) => m.type === "device_offline" && m.device_id === deviceId)
    .then((dropped) => dropped && carrierSwitch.relay(null));

  /** Sever this session deliberately (e.g. switching devices) — no onLost. */
  function close() {
    carrierSwitch.close();
    severed = true;
    rpc.close(new Error("session closed"));
    try {
      ws.close();
    } catch {
      /* already gone */
    }
  }

  return {
    deviceId,
    call: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) =>
      isPaused()
        ? Promise.reject(new Error("your device is offline — reconnecting…"))
        : rpc.call(method, params, { timeoutMs }),
    /** Signaling is pinned to the relay carrier: `rtc.*` never rides the
     *  channel it negotiates, so an ICE restart works while the channels are
     *  down (spec §Signaling). It runs whether or not the app is paused: the
     *  pause holds the user's actions back, and this is the machinery that
     *  looks for a better wire under them. */
    signal: (method, params = {}, timeoutMs = DEFAULT_RPC_TIMEOUT_MS) =>
      rpc.call(method, params, { timeoutMs, carrier: carrierSwitch.relayCarrier() }),
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
