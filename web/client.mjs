// The browser client's E2EE session logic — runtime-agnostic.
//
// It depends only on an injected `transport` (the build-secure-transport JS
// binding) and `send`/`recv` for the relay socket, so the exact same code runs
// in a real browser (native WebSocket) and in the Node end-to-end harness (ws).
//
// Flow: learn the device's transport key → wrap a fresh session key to it →
// verify the device's session_accept → send an encrypted request → read the
// encrypted response. The relay only ever sees opaque envelopes.

// Read frames until the device's key push arrives, skipping relay control
// frames (the post-authenticate `authenticated` ack, other devices' keys).
async function awaitDeviceKey(recv, preferDeviceId = null) {
  for (;;) {
    const message = await recv();
    if (message.type === "device_key" && (!preferDeviceId || message.device_id === preferDeviceId)) {
      return message;
    }
    if (message.type !== "device_key" && message.type !== "authenticated") {
      throw new Error(`expected device_key, got ${message.type}`);
    }
  }
}

// Open an E2EE session and return an RPC `call(method, params)` over it. This is
// the real client API the UI uses: bootstrap once, then make many encrypted calls
// to the bridge's application RPC (task.dispatch, task.diff, task.approve_merge…).
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

  const accept = await recv();
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

    const resp = await recv();
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
  const accept = await recv();
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
  const response = await recv();
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
