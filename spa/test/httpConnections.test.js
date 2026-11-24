// @vitest-environment jsdom
// @vitest-environment-options {"url":"https://127.0.0.1:9443/app/"}
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import {
  cachedTerminalTransport, deviceTransportFor, httpEndpointFor, httpPreferenceFor, rememberHttpTerminalMode, resetHttpConnections, restoreHttpConnections, selectHttpConnection,
} from "../src/core/httpConnections.js";
import { App } from "../src/appState.js";
import { writeUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";

const issuer = location.origin;
const origin = "https://127.0.0.1:8443";
const account = "dc829a54-f3cc-4fa2-be83-a357ff1eadd5";
const deviceId = "device";
const json = (value) => new Response(JSON.stringify(value));

afterEach(() => {
  resetHttpConnections();
  App.devices = [];
  App.deviceTransportPolicy = null;
  delete window.buildBridgeHttp;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
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

const project = (address, assignmentRevision) => {
  App.devices = [{ id: deviceId, fingerprint: "paired-fingerprint", transport: {
    source: "hosted", rtc_allowed: false, origin: address, reason: null, assignment_revision: assignmentRevision,
  } }];
};

it("restores terminal compatibility offline for the current account and endpoint only", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  project(origin, 1);
  await restoreHttpConnections();
  expect(cachedTerminalTransport(deviceId)).toBe("unknown");
  await rememberHttpTerminalMode(deviceId, "wss", () => true);
  resetHttpConnections();
  await restoreHttpConnections();
  expect(cachedTerminalTransport(deviceId)).toBe("wss");
  project("https://127.0.0.1:8444", 2);
  expect(cachedTerminalTransport(deviceId)).toBe("unknown");
  await rememberHttpTerminalMode(deviceId, "update_required", () => true);
  resetHttpConnections();
  await restoreHttpConnections();
  expect(cachedTerminalTransport(deviceId)).toBe("update_required");
  document.head.querySelector("meta").content = "10000000-0000-4000-8000-000000000078";
  App.accountEpoch += 1;
  await restoreHttpConnections();
  expect(cachedTerminalTransport(deviceId)).toBe("unknown");
});

it("uses native discovery for the cached projection and closes a grant that arrives after an account change", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  project(origin, 1);
  const grant = Promise.withResolvers();
  const api = window.buildBridgeHttp = {
    close: vi.fn(async () => ({ ok: true })),
    discoverSaved: vi.fn(() => grant.promise),
  };
  await restoreHttpConnections();
  const pending = httpEndpointFor(deviceId, () => true);
  const rejected = expect(pending).rejects.toMatchObject({ code: "scope_ended" });
  expect(api.discoverSaved).toHaveBeenCalledWith(origin, { deviceId, fingerprint: "paired-fingerprint" });
  App.accountEpoch += 1;
  grant.resolve({ ok: true, value: { deviceId, origin, fingerprint: "paired-fingerprint", handle: "retired" } });
  await rejected;
  expect(api.close).toHaveBeenCalledOnce();
  expect(api.close).toHaveBeenCalledWith("retired");
});

it("holds an HTTPS projection that is still in memory after its account changes", async () => {
  const nextAccount = "10000000-0000-4000-8000-000000000078";
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  project(origin, 1);
  App.deviceTransportPolicy = { accountId: account, policyAware: true };
  expect(deviceTransportFor(deviceId).kind).toBe("http");

  document.head.querySelector("meta").content = nextAccount;
  await restoreHttpConnections();
  const request = vi.fn();
  vi.stubGlobal("fetch", request);
  expect(deviceTransportFor(deviceId)).toMatchObject({ kind: "blocked", reason: "policy_unknown" });
  await expect(httpEndpointFor(deviceId, () => true)).rejects.toMatchObject({ code: "scope_ended" });
  expect(request).not.toHaveBeenCalled();
});

it("gives an old desktop preload upgrade guidance without trying browser discovery", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  project(origin, 1);
  window.buildBridgeHttp = { discover: vi.fn() };
  const request = vi.fn();
  vi.stubGlobal("fetch", request);
  await restoreHttpConnections();
  await expect(httpEndpointFor(deviceId, () => true)).rejects.toMatchObject({ code: "desktop_update_required" });
  expect(request).not.toHaveBeenCalled();
  expect(window.buildBridgeHttp.discover).not.toHaveBeenCalled();
});

it("gives Electron with no preload facade upgrade guidance without trying browser discovery", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  project(origin, 1);
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 Electron/36.0.0");
  expect(window.buildBridgeHttp).toBeUndefined();
  const request = vi.fn();
  vi.stubGlobal("fetch", request);
  await restoreHttpConnections();
  await expect(httpEndpointFor(deviceId, () => true)).rejects.toMatchObject({
    code: "desktop_update_required", message: "Update the Build desktop app to use this HTTPS connection.",
  });
  expect(request).not.toHaveBeenCalled();
});

it("uses the projected browser origin even when a different manual address was saved", async () => {
  document.head.innerHTML = `<meta name="build-account-id" content="${account}">`;
  await restoreHttpConnections();
  await selectHttpConnection({ deviceId, origin: "https://127.0.0.1:8444" });
  project(origin, 1);
  const discoveryPath = issuer + "/api/devices/device/http-discovery";
  const wellKnownPath = origin + "/.well-known/build-bridge";
  const completePath = issuer + "/api/bridge-discovery/complete";
  const request = vi.fn(async (url) => {
    if (url === discoveryPath) return json({
      origin, issuer, account_id: account, device_id: deviceId, challenge_id: "nonce-id", nonce: "nonce",
      expires_at: Date.now() / 1000 + 60, resume_url: "/app/?bridge_discovery=nonce-id",
    });
    if (url === wellKnownPath) return json({ proof: "signed" });
    if (url === completePath) return json({
      origin, issuer, account_id: account, device_id: deviceId, endpoint_epoch: "epoch", family_id: "family",
      fingerprint: "paired-fingerprint", access_token: "fixture-credential", expires_at: Date.now() / 1000 + 300,
      scope: "bridge:read bridge:write",
    });
    throw new Error("Unexpected fixture URL");
  });
  vi.stubGlobal("fetch", request);
  const endpoint = await httpEndpointFor(deviceId, () => true);
  expect(endpoint.identity).toMatchObject({ deviceId, origin });
  expect(request.mock.calls.map(([url]) => url)).toEqual([discoveryPath, wellKnownPath, completePath]);
  endpoint.close();
});
