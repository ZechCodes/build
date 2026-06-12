// The browser client's E2EE session logic — runtime-agnostic.
//
// It depends only on an injected `transport` (the build-secure-transport JS
// binding) and `send`/`recv` for the relay socket, so the exact same code runs
// in a real browser (native WebSocket) and in the Node end-to-end harness (ws).
//
// Flow: learn the device's transport key → wrap a fresh session key to it →
// verify the device's session_accept → send an encrypted request → read the
// encrypted response. The relay only ever sees opaque envelopes.

export async function runClientSession({
  send,
  recv,
  transport,
  deviceId = "dev-relay-device",
  request = { method: "ping", n: 1 },
  log = () => {},
}) {
  if (transport.ready) await transport.ready();

  // 1. The relay hands us the device's transport public key.
  const hello = await recv();
  if (hello.type !== "device_key") {
    throw new Error(`expected device_key, got ${hello.type}`);
  }
  const deviceTransportPublicKeyB64 = hello.transport_public_key;
  log(`device transport key: ${deviceTransportPublicKeyB64.slice(0, 12)}…`);

  // 2. Wrap a fresh session key to the device and open the session.
  const sessionId = "sess-" + Math.random().toString(36).slice(2, 10);
  const { sessionKeyB64, sessionInit } = await transport.createSessionInit({
    sessionId,
    deviceId,
    deviceTransportPublicKeyB64,
  });
  send({ type: "session_init", session_id: sessionId, session_init: sessionInit });

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
