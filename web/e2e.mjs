// Node end-to-end harness: runs the real browser-client logic (client.mjs) and
// the real build-secure-transport JS binding against a live api + relay + bridge.
//
// The shortest proof of the whole transport: dummy-login to the api, mint a
// gateway token, open the relay as a rendezvous, negotiate this device's peer
// connection over it, close the socket, and round-trip one encrypted call over
// the `app` DataChannel. If any of that is broken there is no path to the
// device at all — there is nothing under the peer connection (rules 1–4).
//
// Usage: API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node e2e.mjs

import * as transport from "@build/secure-transport";
import { openDeviceLink, openRendezvous } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const relayUrl = process.env.RELAY_URL || process.env.DEV_RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
// With several devices online, pin the one under test (default: first to answer).
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;

const fail = (message, detail) => {
  console.error("E2E FAIL:", message, detail ?? "");
  process.exit(1);
};

try {
  const { cookie, mintGatewayToken } = await loginWithDummy(apiUrl, { email: process.env.QA_EMAIL || "qa@localhost" });
  const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken });
  if (!rendezvous.authenticated) fail("relay refused the gateway token:", rendezvous.ack);

  const link = await openDeviceLink({ rendezvous, transport, apiUrl, cookie, preferDeviceId });
  console.log(`[client] peer connection up to ${link.device.name} (${link.device.deviceId})`);
  if (!rendezvous.isClosed()) fail("the relay socket is still open under a live peer connection");
  console.log("[client] rendezvous closed; the channels are the only wire");

  const pong = await link.session.call("ping");
  if (pong?.pong !== true) fail("unexpected reply", pong);

  link.close();
  console.log("E2E PASS: browser client ↔ DataChannel ↔ bridge round-trip succeeded");
  process.exit(0);
} catch (err) {
  fail(err.message);
}
