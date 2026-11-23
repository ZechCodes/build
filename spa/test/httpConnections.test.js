// @vitest-environment jsdom
// @vitest-environment-options {"url":"https://127.0.0.1:9443/app/"}
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import {
  httpEndpointFor, httpPreferenceFor, resetHttpConnections, restoreHttpConnections, selectHttpConnection,
} from "../src/core/httpConnections.js";

const issuer = location.origin;
const origin = "https://127.0.0.1:8443";
const account = "dc829a54-f3cc-4fa2-be83-a357ff1eadd5";
const deviceId = "device";
const json = (value) => new Response(JSON.stringify(value));

afterEach(() => {
  resetHttpConnections();
  vi.unstubAllGlobals();
  document.head.replaceChildren();
});

it("persists an address first and renews the verified family after restoring it", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  await restoreHttpConnections();
  await selectHttpConnection({ deviceId, origin });
  expect(httpPreferenceFor(deviceId)).toEqual({ deviceId, origin, selected: "http" });

  const paths = [];
  vi.stubGlobal("fetch", vi.fn(async (url) => {
    paths.push(url);
    if (url === issuer + "/api/devices/device/http-discovery") return json({
      origin, issuer, account_id: account, device_id: deviceId, challenge_id: "nonce-id", nonce: "nonce",
      expires_at: Date.now() / 1000 + 60, resume_url: "/app/?bridge_discovery=nonce-id",
    });
    if (url === origin + "/.well-known/build-bridge") return json({ proof: "signed" });
    if ([issuer + "/api/bridge-discovery/complete", issuer + "/api/devices/device/http-credentials"].includes(url)) {
      return json({
        origin, issuer, account_id: account, device_id: deviceId, endpoint_epoch: "epoch", family_id: "family",
        access_token: "fixture-credential", expires_at: Date.now() / 1000 + 300, scope: "bridge:read bridge:write",
      });
    }
    throw new Error("Unexpected fixture URL");
  }));
  const first = await httpEndpointFor(deviceId, () => true);
  expect(httpPreferenceFor(deviceId)).toMatchObject({ origin, familyId: "family", endpointEpoch: "epoch" });
  first.close();
  resetHttpConnections();
  await restoreHttpConnections();

  // The normal restoration path reopens from the issued family without
  // requiring discovery or a second user action.
  const second = await httpEndpointFor(deviceId, () => true);
  expect(second.identity).toMatchObject({ origin, familyId: "family" });
  expect(paths).toEqual([
    issuer + "/api/devices/device/http-discovery", origin + "/.well-known/build-bridge",
    issuer + "/api/bridge-discovery/complete", issuer + "/api/devices/device/http-credentials",
  ]);
  second.close();
});
