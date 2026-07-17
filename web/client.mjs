// The browser client's E2EE session logic — runtime-agnostic.
//
// It depends only on an injected `transport` (the build-secure-transport JS
// binding) and `send`/`recv` for the relay socket, so the exact same code runs
// in a real browser (native WebSocket) and in the Node end-to-end harness (ws).
//
// Flow: learn the device's transport key → wrap a fresh session key to it →
// verify the device's session_accept → send an encrypted request → read the
// encrypted response. The relay only ever sees opaque envelopes.

// Relay control frames that can arrive interleaved with the protocol flow:
// the post-authenticate ack and the per-device liveness/key pushes.
const RELAY_CONTROL_FRAMES = new Set([
  "authenticated",
  "device_key",
  "device_online",
  "device_offline",
]);

// Read frames until something that is NOT a relay control push arrives.
async function nextProtocolFrame(recv) {
  for (;;) {
    const message = await recv();
    if (!RELAY_CONTROL_FRAMES.has(message.type)) return message;
  }
}

// Read frames until the device's key push arrives, skipping other relay control
// frames (the `authenticated` ack, liveness pushes, other devices' keys).
async function awaitDeviceKey(recv, preferDeviceId = null) {
  for (;;) {
    const message = await recv();
    if (message.type === "device_key" && (!preferDeviceId || message.device_id === preferDeviceId)) {
      return message;
    }
    if (!RELAY_CONTROL_FRAMES.has(message.type)) {
      throw new Error(`expected device_key, got ${message.type}`);
    }
  }
}

// Open an E2EE session and return an RPC `call(method, params)` over it. This is
// the real client API the UI uses: bootstrap once, then make many encrypted calls
// to the bridge's application RPC (plan.create, run.create, run.diff,
// run.git_action…).
export async function openSession({ send, recv, transport, preferDeviceId = null }) {
  if (transport.ready) await transport.ready();

  const hello = await awaitDeviceKey(recv, preferDeviceId);
  const deviceId = hello.device_id;
  const deviceTransportPublicKeyB64 = hello.transport_public_key;

  const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({
    sessionId,
    deviceId,
    deviceTransportPublicKeyB64,
  });
  send({ type: "session_init", session_id: sessionId, route_to: `device:${deviceId}`, session_init: sessionInit });

  const accept = await nextProtocolFrame(recv);
  if (accept.type !== "session_accept") {
    throw new Error(`expected session_accept, got ${accept.type}`);
  }
  await transport.openSessionAccept({ sessionKeyB64, envelope: accept.envelope });

  let reqId = 0;
  async function call(method, params = {}) {
    const id = "r" + ++reqId;
    const envelope = await transport.encryptFrame({
      sessionKeyB64,
      outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
      frameFields: { frame_type: "data", sender: "client", payload: { method, id, params } },
    });
    send({ type: "e2ee_envelope", session_id: sessionId, envelope });

    const resp = await nextProtocolFrame(recv);
    if (resp.type !== "e2ee_envelope") {
      throw new Error(`expected e2ee_envelope, got ${resp.type}`);
    }
    const frame = await transport.decryptEnvelope({ sessionKeyB64, envelope: resp.envelope });
    if (!frame.payload.ok) {
      throw new Error(`RPC ${method} failed: ${frame.payload.error}`);
    }
    return frame.payload.result;
  }

  return { sessionId, call };
}

// Open an E2EE session with a background receive loop that routes decrypted
// payloads: those with `id`+`ok` resolve pending `call`s, those with a `type`
// (term.output / term.reset / term.closed) go to `onPush`. This mirrors
// production's dedicated terminal socket — one connection carrying both the
// request/response terminal RPCs and the server-initiated PTY pushes. The main
// `openSession` stays strictly request-response.
export async function openPushSession({ send, recv, transport, preferDeviceId = null, onPush = () => {} }) {
  if (transport.ready) await transport.ready();

  const hello = await awaitDeviceKey(recv, preferDeviceId);
  const deviceId = hello.device_id;
  const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({
    sessionId,
    deviceId,
    deviceTransportPublicKeyB64: hello.transport_public_key,
  });
  send({ type: "session_init", session_id: sessionId, route_to: `device:${deviceId}`, session_init: sessionInit });

  const accept = await nextProtocolFrame(recv);
  if (accept.type !== "session_accept") {
    throw new Error(`expected session_accept, got ${accept.type}`);
  }
  await transport.openSessionAccept({ sessionKeyB64, envelope: accept.envelope });

  const pending = new Map();
  let reqId = 0;

  // Background demux: every frame from here on is either an RPC response or a
  // server push. A recv timeout/close ends the loop and rejects stragglers.
  (async () => {
    for (;;) {
      let msg;
      try {
        msg = await recv();
      } catch {
        for (const { reject } of pending.values()) reject(new Error("terminal session closed"));
        pending.clear();
        return;
      }
      if (!msg || msg.type !== "e2ee_envelope") continue;
      let frame;
      try {
        frame = await transport.decryptEnvelope({ sessionKeyB64, envelope: msg.envelope });
      } catch {
        continue;
      }
      const p = frame.payload;
      if (p && p.id !== undefined && p.ok !== undefined) {
        const waiter = pending.get(p.id);
        if (waiter) {
          pending.delete(p.id);
          p.ok ? waiter.resolve(p.result) : waiter.reject(new Error(p.error));
        }
      } else if (p && p.type) {
        onPush(p);
      }
    }
  })();

  function call(method, params = {}) {
    const id = "r" + ++reqId;
    let reject;
    const result = new Promise((res, rej) => {
      reject = rej;
      pending.set(id, { resolve: res, reject: rej });
    });
    transport
      .encryptFrame({
        sessionKeyB64,
        outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
        frameFields: { frame_type: "data", sender: "client", payload: { method, id, params } },
      })
      .then((envelope) => send({ type: "e2ee_envelope", session_id: sessionId, envelope }))
      .catch((e) => {
        pending.delete(id);
        reject(e);
      });
    return result;
  }

  return { sessionId, call };
}

export async function runClientSession({
  send,
  recv,
  transport,
  preferDeviceId = null,
  request = { method: "ping", n: 1 },
  log = () => {},
}) {
  if (transport.ready) await transport.ready();

  // 1. The relay hands us the device's transport public key.
  const hello = await awaitDeviceKey(recv, preferDeviceId);
  const deviceId = hello.device_id;
  const deviceTransportPublicKeyB64 = hello.transport_public_key;
  log(`device transport key: ${deviceTransportPublicKeyB64.slice(0, 12)}…`);

  // 2. Wrap a fresh session key to the device and open the session.
  const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({
    sessionId,
    deviceId,
    deviceTransportPublicKeyB64,
  });
  send({ type: "session_init", session_id: sessionId, route_to: `device:${deviceId}`, session_init: sessionInit });

  // 3. The device proves it unwrapped the key with an encrypted session_accept.
  const accept = await nextProtocolFrame(recv);
  if (accept.type !== "session_accept") {
    throw new Error(`expected session_accept, got ${accept.type}`);
  }
  await transport.openSessionAccept({ sessionKeyB64, envelope: accept.envelope });
  log("session established and verified");

  // 4. Send an encrypted request frame.
  const envelope = await transport.encryptFrame({
    sessionKeyB64,
    outerFields: { session_id: sessionId, route_to: `device:${deviceId}` },
    frameFields: { frame_type: "data", sender: "client", payload: request },
  });
  send({ type: "e2ee_envelope", session_id: sessionId, envelope });

  // 5. Read and decrypt the device's response.
  const response = await nextProtocolFrame(recv);
  if (response.type !== "e2ee_envelope") {
    throw new Error(`expected e2ee_envelope, got ${response.type}`);
  }
  const frame = await transport.decryptEnvelope({
    sessionKeyB64,
    envelope: response.envelope,
  });
  log(`decrypted response: ${JSON.stringify(frame.payload)}`);
  return frame.payload;
}
