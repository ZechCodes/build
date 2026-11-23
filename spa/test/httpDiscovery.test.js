import { describe, expect, it, vi } from "vitest";
import { createBridgeDiscovery } from "../src/core/httpDiscovery.js";

const issuer = "https://app.example";
const origin = "https://bridge.example";
const pending = { origin, account_id: "account", issuer, challenge_id: "challenge", nonce: "nonce", expires_at: 1060, resume_url: "/app/?bridge_discovery=challenge" };
const mint = { origin, issuer, account_id: "account", device_id: "device", endpoint_epoch: "epoch", family_id: "family", access_token: "bridge-secret", expires_at: 1300, scope: "bridge:read bridge:write", name: "Laptop", fingerprint: "fp" };
const json = (value) => new Response(JSON.stringify(value), { headers: { "content-type": "application/json" } });
const create = (fetchImpl, isCurrent = () => true) => createBridgeDiscovery({ issuer, accountId: "account", now: () => 1000_000, fetchImpl, isCurrent });

describe("account-bound Add by URL", () => {
  it("does not contact the bridge before a fresh document and hosted completion", async () => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(pending)).mockResolvedValueOnce(json(pending))
      .mockResolvedValueOnce(json({ proof: "signed-proof" })).mockResolvedValueOnce(json(mint));
    const discovery = create(fetchImpl);
    expect(await discovery.begin(origin)).toBe(issuer + pending.resume_url);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    const verified = await discovery.complete("challenge");
    const [url, options] = fetchImpl.mock.calls[2];
    expect(url).toBe(origin + "/.well-known/build-bridge");
    expect(options).toMatchObject({ credentials: "omit", redirect: "error", referrerPolicy: "no-referrer" });
    expect(new Headers(options.headers).has("authorization")).toBe(false);
    expect(options.body).toBe(JSON.stringify({ challenge_id: "challenge", nonce: "nonce" }));
    expect(verified.identity).toMatchObject({ deviceId: "device", origin, endpointEpoch: "epoch", familyId: "family" });
    expect(JSON.stringify(verified.identity)).not.toContain("bridge-secret");
    expect(verified.token()).toBe("bridge-secret");
    verified.close();
    expect(() => verified.token()).toThrow();
  });

  it.each([
    { account_id: "another" }, { issuer: "https://other.example" }, { origin: "https://attacker.example" },
    { expires_at: 999 }, { device_id: "" }, { access_token: "" },
    { scope: "bridge:read" },
  ])("refuses mismatched or invalid minted authority %j", async (change) => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(pending)).mockResolvedValueOnce(json({ proof: "proof" }))
      .mockResolvedValueOnce(json({ ...mint, ...change }));
    await expect(create(fetchImpl).complete("challenge")).rejects.toThrow();
    expect(fetchImpl).toHaveBeenCalledTimes(3);
  });

  it("fences an account change before bridge access", async () => {
    let active = true;
    const fetchImpl = vi.fn().mockImplementation(async () => { active = false; return json(pending); });
    await expect(create(fetchImpl, () => active).complete("challenge")).rejects.toThrow();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("never follows an untrusted resume target or expired pending challenge", async () => {
    for (const change of [{ resume_url: "https://evil.example" }, { expires_at: 999 }]) {
      await expect(create(vi.fn().mockResolvedValue(json({ ...pending, ...change }))).begin(origin)).rejects.toThrow();
    }
  });

  it.each([{ device_id: "other" }, { endpoint_epoch: "other" }, { family_id: "other" }])("refuses a changed renewal binding %j", async (change) => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json({ ...mint, ...change }));
    await expect(create(fetchImpl).resume({ deviceId: "device", origin, endpointEpoch: "epoch", familyId: "family" })).rejects.toThrow();
    expect(fetchImpl.mock.calls[0][0]).toBe(issuer + "/api/devices/device/http-credentials");
    expect(new Headers(fetchImpl.mock.calls[0][1].headers).has("authorization")).toBe(false);
  });

  it.each([408, 429, 503])("keeps hosted renewal retryable after HTTP %s", async (status) => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(new Response(null, { status }));
    await expect(create(fetchImpl).resume({ deviceId: "device", origin, endpointEpoch: "epoch", familyId: "family" }))
      .rejects.toMatchObject({ code: "network" });
  });

  it("ends a stale mint while keeping the saved identity renewable", async () => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json({ ...mint, expires_at: 999 })).mockResolvedValueOnce(json(mint));
    const discovery = create(fetchImpl);
    const identity = { deviceId: "device", origin, endpointEpoch: "epoch", familyId: "family" };
    await expect(discovery.resume(identity)).rejects.toMatchObject({ code: "scope_ended" });
    const renewed = await discovery.resume(identity);
    expect(renewed.identity).toMatchObject(identity);
    expect(renewed.token()).toBe(mint.access_token);
    renewed.close();
  });

  it("recovers a saved device through one bound challenge without its previous family", async () => {
    const saved = { deviceId: "device", origin, endpointEpoch: "old", familyId: "old" };
    const fetchImpl = vi.fn().mockResolvedValueOnce(json({ ...pending, device_id: "device" }))
      .mockResolvedValueOnce(json({ proof: "proof" })).mockResolvedValueOnce(json(mint));
    const discovery = create(fetchImpl);
    const challenge = await discovery.beginSaved(saved);
    const renewed = await discovery.finishSaved(challenge, saved);
    expect(renewed.identity).toMatchObject({ deviceId: "device", origin, endpointEpoch: "epoch", familyId: "family" });
    expect(fetchImpl.mock.calls.map(([url]) => url)).toEqual([
      issuer + "/api/devices/device/http-discovery", origin + "/.well-known/build-bridge",
      issuer + "/api/bridge-discovery/complete",
    ]);
    expect(fetchImpl.mock.calls[1][1]).toMatchObject({ credentials: "omit", redirect: "error" });
    renewed.close();
  });

  it("refuses a saved-device challenge for another device before contacting the bridge", async () => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json({ ...pending, device_id: "other" }));
    await expect(create(fetchImpl).beginSaved({ deviceId: "device", origin })).rejects.toMatchObject({ code: "wrong_bridge" });
    expect(fetchImpl).toHaveBeenCalledOnce();
  });
});
