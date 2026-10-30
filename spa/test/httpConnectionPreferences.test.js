import { describe, expect, it } from "vitest";
import { createHttpConnectionPreferences, httpRecoveryDevices } from "../src/core/httpConnectionPreferences.js";

const identity = { deviceId: "device", origin: "https://bridge.example", endpointEpoch: "epoch", familyId: "family", access_token: "must-not-persist" };
const create = (accountId = "a", initial) => {
  let saved = initial;
  const store = createHttpConnectionPreferences({ accountId, read: async () => saved, write: async (value) => { saved = value; } });
  return { store, saved: () => saved };
};

describe("account-bound transport preference", () => {
  it("stores one hint per paired device and no secret", async () => {
    const { store, saved } = create();
    await store.restore(); await store.selectHttp(identity);
    await store.selectHttp({ ...identity, origin: "https://other.example", endpointEpoch: "new" });
    expect(saved().connections).toHaveLength(1);
    expect(saved().connections[0]).toEqual({ deviceId: "device", origin: "https://other.example", endpointEpoch: "new", familyId: "family", selected: "http" });
    expect(JSON.stringify(saved())).not.toContain("must-not-persist");
    expect(store.forDevice("device").selected).toBe("http");
    await store.selectWebRtc("device");
    expect(store.forDevice("device").selected).toBe("webrtc");
  });

  it("ignores another account's persisted hints and malformed authorities", async () => {
    for (const initial of [{ accountId: "old", connections: [{ ...identity, selected: "http" }] }, { accountId: "a", connections: [{ ...identity, origin: "http://insecure", selected: "http" }] }]) {
      const { store } = create("a", initial); await store.restore();
      expect(store.forDevice("device")).toBeNull();
    }
  });

  it("keeps selected HTTP eligible despite relay flapping without changing account facts", async () => {
    const { store } = create(); await store.restore(); await store.selectHttp(identity);
    const offline = [{ id: "device", status: "offline" }, { id: "other", status: "offline" }];
    expect(httpRecoveryDevices(offline, store.forDevice)).toEqual([{ id: "device", status: "online" }, { id: "other", status: "offline" }]);
    expect(offline[0].status).toBe("offline");
    await store.selectWebRtc("device");
    expect(httpRecoveryDevices(offline, store.forDevice)).toEqual(offline);
    await store.forget("device");
    expect(store.forDevice("device")).toBeNull();
  });
});
