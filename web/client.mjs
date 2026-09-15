// The browser client's session logic, as the Node harnesses run it.
//
// The relay is a rendezvous, not a connection (strict P2P transport spec, rule
// 4): this opens a socket to it, mints the device's sessions on it, negotiates
// the peer connection over it, and **closes it**. Every check above this line
// then runs over the two DataChannels, which is the only thing the browser is
// live over (rule 2) and the only thing the bridge will carry app RPC on (rule
// 1).
//
// The relay also no longer says who is online or what their keys are (rule 6):
// the transport key a session is sealed to is the one the api pinned at
// pairing, read from `GET /api/devices`, and the relay is never asked.
//
// Everything relay-shaped lives in `openRendezvous` below. A direct-network
// mode (LAN, Tailscale) is a second implementation of that one function with
// nothing above it changed — the seam rule 7 reserves.

import WebSocket from "ws";

import { openCarriedSession, openPeerLink, openPeerSession } from "./peer.mjs";

export { openPeerSession };

/** How long the relay has to accept the socket and the device to answer a
 *  `session_init` it has been offered. */
const OPEN_TIMEOUT_MS = 10000;
const ACCEPT_TIMEOUT_MS = 15000;

const newSessionId = () => "sess-" + Math.random().toString(36).slice(2, 10);

/**
 * The rendezvous: one authenticated relay socket, for however many sessions.
 *
 * `onMessage` is a subscription and `recv` is a one-shot wait, because both
 * readers exist here: a mint waits for its own `session_accept`, and a session
 * riding this socket reads every envelope that names it. A socket read by a
 * single-waiter queue would have one session eating another's answer.
 */
export async function openRendezvous({ relayUrl, mintGatewayToken, WebSocketImpl = WebSocket, openTimeoutMs = OPEN_TIMEOUT_MS }) {
  const socket = new WebSocketImpl(`${relayUrl}/ws/client`);
  const listeners = new Set();
  const queue = [];
  const waiters = [];

  socket.on("message", (data) => {
    const message = JSON.parse(data.toString());
    for (const listener of [...listeners]) listener(message);
    waiters.length ? waiters.shift()(message) : queue.push(message);
  });

  const recv = (timeoutMs = ACCEPT_TIMEOUT_MS) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("recv timeout")), timeoutMs);
      const deliver = (message) => {
        clearTimeout(timer);
        resolve(message);
      };
      queue.length ? deliver(queue.shift()) : waiters.push(deliver);
    });

  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("the relay did not answer")), openTimeoutMs);
    socket.on("open", () => (clearTimeout(timer), resolve()));
    socket.on("error", (error) => (clearTimeout(timer), reject(error)));
  });

  socket.send(JSON.stringify({ type: "authenticate", token: await mintGatewayToken() }));
  const ack = await recv();

  return {
    socket,
    ack,
    authenticated: ack.type === "authenticated",
    send: (message) => socket.send(JSON.stringify(message)),
    recv,
    onMessage(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    /** Nothing is negotiating: the relay is not held open between upgrades.
     *  Resolves when the socket is actually shut, not when the close was asked
     *  for — "the relay socket is closed" is a check, and a socket still in
     *  CLOSING would answer it either way depending on the scheduler. */
    close: () =>
      new Promise((resolve) => {
        if (socket.readyState === 3) return resolve();
        socket.on("close", resolve);
        socket.close();
      }),
    isClosed: () => socket.readyState === 3,
  };
}

/** How long to wait for the device under test to report itself online. A bridge
 *  approved a moment ago has not attached to the relay yet, and its heartbeat
 *  is what says it has — the harness's version of the SPA's presence poll,
 *  which is also how a late device joins (rule 6). */
const ONLINE_TIMEOUT_MS = 90000;
const PRESENCE_POLL_MS = 1000;

/**
 * The device this harness is talking to, and the transport key the api pinned
 * for it.
 *
 * Presence and keys are the api's and only the api's: the relay announces
 * neither any more, and is never asked for one. `preferDeviceId` pins one
 * machine when the account has several; otherwise the first one reported
 * online. It polls, because "online" is derived from a heartbeat and a device
 * that is coming up has not sent one yet — and a session offered to a device
 * whose socket is not attached is a `session_init` nobody answers.
 */
export async function pinnedDevice({ apiUrl, cookie, preferDeviceId = null, onlineTimeoutMs = ONLINE_TIMEOUT_MS }) {
  const deadline = Date.now() + onlineTimeoutMs;
  for (;;) {
    const response = await fetch(`${apiUrl}/api/devices`, { headers: { Cookie: cookie } });
    if (!response.ok) throw new Error(`GET /api/devices failed: HTTP ${response.status}`);
    const { devices } = await response.json();
    const owned = devices.filter((device) => device.approved && (!preferDeviceId || device.device_id === preferDeviceId));
    const device = owned.find((candidate) => candidate.status === "online");
    if (device) {
      if (!device.transport_public_key_b64) throw new Error(`device ${device.device_id} has no pinned transport key`);
      return { deviceId: device.device_id, name: device.name, transportPublicKeyB64: device.transport_public_key_b64 };
    }
    if (Date.now() >= deadline) {
      const seen = owned.map((candidate) => `${candidate.name}=${candidate.status}`).join(", ") || "none";
      throw new Error(`no device online for this account${preferDeviceId ? ` matching ${preferDeviceId}` : ""} (${seen})`);
    }
    await new Promise((resolve) => setTimeout(resolve, PRESENCE_POLL_MS));
  }
}

/** One session sealed to the pinned key, minted over the rendezvous. The relay
 *  routes `session_accept` by the session id it was given, which is how several
 *  sessions mint on one socket (rule 5). */
async function mint({ rendezvous, transport, device }) {
  const sessionId = newSessionId();
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({
    sessionId,
    deviceId: device.deviceId,
    deviceTransportPublicKeyB64: device.transportPublicKeyB64,
  });
  const accepted = accept(rendezvous, sessionId);
  rendezvous.send({
    type: "session_init",
    session_id: sessionId,
    route_to: `device:${device.deviceId}`,
    session_init: sessionInit,
  });
  await transport.openSessionAccept({ sessionKeyB64, envelope: (await accepted).envelope });
  return { sessionId, sessionKeyB64, deviceId: device.deviceId };
}

/** This session's `session_accept`, or the reason there is none. */
function accept(rendezvous, sessionId) {
  return new Promise((resolve, reject) => {
    const deadline = setTimeout(() => (stop(), reject(new Error(`device did not accept ${sessionId}`))), ACCEPT_TIMEOUT_MS);
    const stop = rendezvous.onMessage((message) => {
      if (message.type !== "session_accept" || message.session_id !== sessionId) return;
      clearTimeout(deadline);
      stop();
      resolve(message);
    });
  });
}

/** One session's lease on the relay socket. It is not the socket's owner: the
 *  rendezvous mints several sessions on one wire, so letting a carrier go
 *  leaves the socket to the others. Only the rendezvous closes it. */
function relayCarrier(rendezvous, sessionId) {
  return {
    send: (envelope) => rendezvous.send({ type: "e2ee_envelope", session_id: sessionId, envelope }),
    onEnvelope: (fn) =>
      rendezvous.onMessage((message) => {
        if (message.type !== "e2ee_envelope") return;
        if (message.session_id && message.session_id !== sessionId) return;
        fn(message.envelope);
      }),
    close: () => {},
  };
}

/**
 * A session whose carrier is the relay socket itself.
 *
 * This is the signaling wire — `rtc.*` and nothing else — and it is the one
 * place a harness can ask the bridge to break rule 1 and watch it refuse.
 * Nothing else may run over it: the refusal is `error_code: "unavailable"` with
 * `details.reason === "relay_is_not_a_data_plane"`, and it is not retryable.
 */
export async function openRelaySignalingSession({ rendezvous, transport, device, onPush, onFrame }) {
  const minted = await mint({ rendezvous, transport, device });
  return openCarriedSession({
    carrier: relayCarrier(rendezvous, minted.sessionId),
    transport,
    ...minted,
    onPush,
    onFrame,
  });
}

/**
 * The whole connect sequence for one device, as `spa/src/connection.js` runs it.
 *
 * Open the rendezvous → mint this device's sessions on it → negotiate the peer
 * connection over it → put each session on its channel → **close the relay
 * socket**. What comes back is carried by the DataChannels and nothing else;
 * `rendezvous.isClosed()` is true for the rest of the run.
 *
 * `terminal` asks for the second session rule 5 describes — the terminals' one,
 * minted on the same socket and riding this device's `term` channel, never a
 * socket of its own.
 */
export async function openDeviceLink({
  rendezvous,
  transport,
  apiUrl,
  cookie,
  preferDeviceId = null,
  onPush = () => {},
  onFrame = () => {},
  terminal = null,
}) {
  if (transport.ready) await transport.ready();
  const device = await pinnedDevice({ apiUrl, cookie, preferDeviceId });

  const appMint = await mint({ rendezvous, transport, device });
  const termMint = terminal ? await mint({ rendezvous, transport, device }) : null;

  const signaling = openCarriedSession({
    carrier: relayCarrier(rendezvous, appMint.sessionId),
    transport,
    ...appMint,
  });
  const link = await openPeerLink({
    signal: signaling.call,
    onSignalPush: signaling.onPush,
    apiUrl,
    cookie,
  });

  const session = openPeerSession({ carrier: link.app, transport, ...appMint, onPush, onFrame });
  const terminalSession = termMint
    ? openPeerSession({ carrier: link.term, transport, ...termMint, onPush: terminal.onPush, onFrame: terminal.onFrame })
    : null;

  // Each session rides its channel before the socket goes: a session whose last
  // carrier ends is a session the bridge has ended, and until its first frame
  // crosses a channel the relay carrier is the only one it has.
  await session.call("ping");
  if (terminalSession) await terminalSession.call("ping");

  await rendezvous.close();
  return { device, session, terminalSession, link, close: () => link.close() };
}

/** The peer-backed session alone — the harness's `openSession`, for a check
 *  that needs one device and no terminal. */
export async function openSession(options) {
  return (await openDeviceLink(options)).session;
}
