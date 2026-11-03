import assert from "node:assert/strict";
import test from "node:test";
import { createBridgeHttp } from "../src/bridge-http.mjs";

const localIssuer = "https://localhost:18476";
const bridgeOrigin = "https://localhost:19476";
const json = (value) => new Response(JSON.stringify(value), {
  headers: { "content-type": "application/json" },
});
function harness(t, issuer = localIssuer) {
  const requests = [];
  const owner = {};
  const bridge = createBridgeHttp({
    hostedOrigin: localIssuer,
    fetch: async (url) => {
      requests.push(url);
      if (url.endsWith("/challenges")) return json({
        issuer: localIssuer, challenge_id: "challenge", nonce: "n".repeat(32),
        expires_at: Math.floor(Date.now() / 1000) + 60,
      });
      if (url.endsWith("/.well-known/build-bridge")) return json({ proof: "signed-proof" });
      return json({
        issuer, account_id: "account", device_id: "device", origin: bridgeOrigin,
        endpoint_epoch: "epoch", family_id: "family", access_token: "fixture-device-token",
        expires_at: Math.floor(Date.now() / 1000) + 299, scope: "bridge:read bridge:write",
      });
    },
    getAuth: () => ({ generation: 1, accessToken: "fixture-hosted-token" }),
    isOwnerCurrent: () => true,
    send: () => {},
    manifest: { version: 1, operations: [] },
  });
  t.after(() => bridge.clear());
  return { bridge, owner, requests };
}

test("main-process hosted origin selects discovery routes and exact credential issuer", async (t) => {
  const h = harness(t);
  const identity = await h.bridge.discover(h.owner, bridgeOrigin);
  assert.equal(identity.origin, bridgeOrigin);
  assert.deepEqual(h.requests, [
    `${localIssuer}/api/bridge-discovery/challenges`,
    `${bridgeOrigin}/.well-known/build-bridge`,
    `${localIssuer}/api/bridge-discovery/complete`,
  ]);
});

test("a local hosted origin never accepts the production issuer as a substitute", async (t) => {
  const h = harness(t, "https://getbuild.ing");
  await assert.rejects(h.bridge.discover(h.owner, bridgeOrigin), { code: "binding_mismatch" });
});

test("main-process hosted origin also binds resume and family cleanup", async (t) => {
  const h = harness(t);
  await h.bridge.resume(h.owner, {
    deviceId: "device", origin: bridgeOrigin, familyId: "family", endpointEpoch: "epoch",
  });
  assert.deepEqual(await h.bridge.revoke("fixture-hosted-token"), { failed: 0 });
  assert.deepEqual(h.requests, [
    `${localIssuer}/api/devices/device/http-credentials`,
    `${localIssuer}/api/devices/device/http-credentials/family`,
  ]);
});

for (const hostedOrigin of ["http://localhost:18476", "https://localhost:18476/path", "https://user@localhost:18476"]) {
  test(`main-process hosted origin refuses ${hostedOrigin}`, () => {
    assert.throws(() => createBridgeHttp({ hostedOrigin }), { code: "invalid_origin" });
  });
}

for (const status of [401, 403, 429, 503]) {
  test(`discovery completion ${status} distinguishes proof refusal from hosted service failure`, async (t) => {
    const owner = {};
    const bridge = createBridgeHttp({
      hostedOrigin: localIssuer,
      getAuth: () => ({ generation: 1, accessToken: "fixture-hosted-token" }),
      isOwnerCurrent: () => true, send: () => {}, manifest: { version: 1, operations: [] },
      fetch: async (url) => {
        if (url.endsWith("/challenges")) return json({
          issuer: localIssuer, challenge_id: "challenge", nonce: "n".repeat(32),
          expires_at: Math.floor(Date.now() / 1000) + 60,
        });
        if (url.endsWith("/.well-known/build-bridge")) return json({ proof: "tampered-proof" });
        return new Response(null, { status });
      },
    });
    t.after(() => bridge.clear());
    const refusedProof = status === 401 || status === 403;
    await assert.rejects(bridge.discover(owner, bridgeOrigin), refusedProof ? {
      code: "discovery_failed",
      message: "Build could not verify this HTTPS address. Check the address and try again.",
    } : {
      code: "http_error", status,
      message: "Bridge HTTP http error.",
    });
  });
}
