// @vitest-environment jsdom
// @vitest-environment-options {"url":"https://127.0.0.1:9443/app/"}
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import {
  httpEndpointFor, httpPreferenceFor, resetHttpConnections, restoreHttpConnections, selectHttpConnection,
} from "../src/core/httpConnections.js";
import { writeUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";

const issuer = location.origin;
const origin = "https://127.0.0.1:8443";
const account = "dc829a54-f3cc-4fa2-be83-a357ff1eadd5";
const deviceId = "device";
const json = (value) => new Response(JSON.stringify(value));

afterEach(() => {
  resetHttpConnections();
  delete window.buildBridgeHttp;
  vi.unstubAllGlobals();
  document.head.replaceChildren();
});

it.each([false, true])("preserves the learned desktop pin across launch, with legacy grant hint %s", async (hasHint) => {
  const desktopAccount = hasHint ? "84fa847a-f31e-412d-a8e1-fefbf560c25d" : "84fa847a-f31e-412d-a8e1-fefbf560c25e";
  document.head.innerHTML = `<meta name="build-account-id" content="${desktopAccount}">`;
  const legacy = { deviceId, origin, selected: "http" };
  let saved = legacy;
  const identity = { deviceId, origin, fingerprint: "paired-fingerprint", familyId: "family", endpointEpoch: "epoch", handle: "handle" };
  if (hasHint) {
    const address = uiAddress({ entityId: desktopAccount, view: "account", kind: "http-connections" });
    await writeUiRecord(address, { accountId: desktopAccount, connections: [
      { ...legacy, familyId: identity.familyId, endpointEpoch: identity.endpointEpoch },
    ] });
  }
  const approved = { ok: true, value: identity };
  const api = window.buildBridgeHttp = {
    settingsRead: vi.fn(async () => ({ ok: true, value: { accountId: desktopAccount, connections: [saved] } })),
    settingsWrite: vi.fn(async ({ setting }) => { saved = setting; return { ok: true, value: true }; }),
    discoverSaved: vi.fn(async () => approved),
    resume: vi.fn(async () => approved),
    close: vi.fn(async () => ({ ok: true })),
  };
  await restoreHttpConnections();
  (await httpEndpointFor(deviceId, () => true)).close();
  expect(api.settingsWrite).toHaveBeenCalledWith({
    accountId: desktopAccount, deviceId, setting: { ...legacy, fingerprint: identity.fingerprint }, expected: legacy,
  });
  resetHttpConnections();
  await restoreHttpConnections();
  (await httpEndpointFor(deviceId, () => true)).close();
  expect(api.discoverSaved).toHaveBeenCalledTimes(hasHint ? 0 : 1);
  expect(api.resume).toHaveBeenCalledWith({
    deviceId, origin, fingerprint: identity.fingerprint, familyId: "family", endpointEpoch: "epoch",
  });
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
