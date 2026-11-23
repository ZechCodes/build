// @vitest-environment jsdom
// @vitest-environment-options {"url":"https://127.0.0.1:9443/app/"}
import { afterEach, expect, it, vi } from "vitest";
import { openSavedBrowserGrant } from "../src/core/httpBrowserRecovery.js";
import { checkHttpNetworkPermission, requestHttpNetworkPermission } from "../src/core/httpNetworkPermission.js";

const issuer = location.origin;
const origin = "https://127.0.0.1:8443";
const identity = { deviceId: "device", origin, endpointEpoch: "old", familyId: "revoked", fingerprint: "paired" };
const args = { identity, accountId: "account", devices: ["device"], current: () => true };
const json = (value, status = 200) => new Response(JSON.stringify(value), { status });
const challenge = () => ({
  origin, issuer, device_id: "device", account_id: "account", challenge_id: "challenge",
  nonce: "nonce", expires_at: Date.now() / 1000 + 60, resume_url: "/app/?bridge_discovery=challenge",
});
const mint = () => ({
  origin, issuer, device_id: "device", account_id: "account", fingerprint: "paired",
  endpoint_epoch: "new", family_id: "fresh", access_token: "fixture-bearer",
  scope: "bridge:read bridge:write", expires_at: Date.now() / 1000 + 300,
});

afterEach(() => vi.unstubAllGlobals());

it("uses the signed Add by URL proof to replace a refused saved family", async () => {
  const network = vi.fn().mockResolvedValueOnce(json({}, 401)).mockResolvedValueOnce(json(challenge()))
    .mockResolvedValueOnce(json({ proof: "signed-proof" })).mockResolvedValueOnce(json(mint()));
  vi.stubGlobal("fetch", network);
  const grant = await openSavedBrowserGrant(args);
  expect(grant.identity).toMatchObject({ deviceId: "device", origin, endpointEpoch: "new", familyId: "fresh" });
  expect(network.mock.calls.map(([url]) => url)).toEqual([
    issuer + "/api/devices/device/http-credentials", issuer + "/api/devices/device/http-discovery",
    origin + "/.well-known/build-bridge", issuer + "/api/bridge-discovery/complete",
  ]);
  expect(network.mock.calls[2][1]).toMatchObject({ credentials: "omit", redirect: "error" });
  expect(new Headers(network.mock.calls[2][1].headers).has("Authorization")).toBe(false);
  grant.close();
});

it("keeps the origin and honors browser permission before reaching it", async () => {
  const permission = { state: "prompt" };
  const query = vi.fn(async () => permission);
  const network = vi.fn().mockResolvedValueOnce(json({}, 405)).mockResolvedValueOnce(json(mint()));
  vi.stubGlobal("fetch", network);
  const check = () => checkHttpNetworkPermission(origin, { query }, "https://public.example");
  await expect(check()).rejects.toMatchObject({ code: "network_permission" });
  expect(query).toHaveBeenCalledWith({ name: "loopback-network" });
  expect(network).not.toHaveBeenCalled();
  await requestHttpNetworkPermission(origin);
  expect(network).toHaveBeenCalledWith(origin + "/.well-known/build-bridge",
    expect.objectContaining({ method: "GET", credentials: "omit", redirect: "error" }));
  permission.state = "granted";
  await expect(check()).resolves.toBeUndefined();
  const grant = await openSavedBrowserGrant({ ...args, identity: { ...identity, endpointEpoch: "new", familyId: "fresh" } });
  expect(grant.identity.origin).toBe(origin);
  grant.close();
});

it.each([[401, "sign_in"], [503, "network"]])("keeps saved settings when hosted responds %s", async (status, code) => {
  const network = vi.fn().mockResolvedValueOnce(json({}, 401)).mockResolvedValueOnce(json({}, status));
  vi.stubGlobal("fetch", network);
  await expect(openSavedBrowserGrant(args)).rejects.toMatchObject({ code });
  expect(network.mock.calls.every(([url]) => url.startsWith(issuer + "/"))).toBe(true);
});
