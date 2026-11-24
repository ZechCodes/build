// @vitest-environment jsdom
// The account's device list is the one thing in the cache that is nobody's
// device: it comes from `GET /api/devices` on skriftapp, not from a bridge.
// The presence poll is the only reader of it, so the poll is what writes it —
// and it is written so a reload paints the rail's machines before the first
// read of the account has answered.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const account = vi.hoisted(() => ({ fetchDevices: vi.fn() }));

vi.mock("../src/api.js", () => ({ fetchDevices: (...args) => account.fetchDevices(...args) }));
vi.mock("../src/connection.js", () => ({
  chooseDeviceTransport: async () => null,
  deviceRecoverySnapshot: () => [],
  onDeviceRecoveryChanged: () => () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  deviceWentAway: () => {},
  syncHome: () => {},
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {},
  chooseCreationDevice: () => {},
  connectDevice: () => Promise.resolve(null),
  goOffline: () => {},
  retireDevice: () => {},
  openDeviceSettingsSession: async () => ({}),
  forgetHomeFollow: () => {},
  forgetRendezvousSockets: () => {},
  forgetSecurityStops: () => {},
  securityStopText: () => "",
}));

const { App } = await import("../src/app.js");
const { initDevicePicker, readCachedDevices, refreshDevices } = await import("../src/devices.js");
// The one address, read from where both sides of it read it: the writer here
// and the boot paint (views/gate.js) agree on it because there is one
// constant, and a test that restated it would stay green while they drifted.
const { DEVICES_ADDRESS, mergeCachedTogether, readCached, takeCachedCount, wipeCache, writeCached } =
  await import("../src/core/localCache.js");

async function cacheTestState() {
  App.devices = [];
  App.deviceTransportPolicy = null;
  await readCachedDevices({ offline: true });
  expect(App.devices).toMatchObject([{ id: "current", status: "offline" }]);
  expect(App.deviceTransportPolicy.promotionRevision).toBe(3);
}

const online = (id) => ({ id, name: id, status: "online", fingerprint: `${id}-fp` });
const firstAccount = "10000000-0000-4000-8000-000000000054";
const nextAccount = "10000000-0000-4000-8000-000000000055";
const setAccount = (id) => { document.head.innerHTML = `<meta name="build-account-id" content="${id}">`; };
const hosted = (id, port, revision) => ({ ...online(id), transport: {
  source: "hosted", rtc_allowed: false, origin: `https://127.0.0.1:${port}`, reason: null, assignment_revision: revision,
} });

beforeEach(() => {
  document.body.innerHTML = '<div id="devpick"></div>';
  App.devices = [];
  App.accountEpoch = 0;
  App.gated = false;
  App.deviceFilter = null;
  account.fetchDevices.mockReset();
});

afterEach(async () => {
  await wipeCache();
  document.head.replaceChildren();
  App.deviceTransportPolicy = null;
});

describe("the account's device list on disk", () => {
  it("is written by the read that answers it", async () => {
    account.fetchDevices.mockResolvedValue([online("dev-a"), online("dev-b")]);
    await refreshDevices();
    expect((await readCached(DEVICES_ADDRESS)).value.map((device) => device.id)).toEqual(["dev-a", "dev-b"]);
  });

  it("is replaced wholesale — a machine the account stopped listing leaves it", async () => {
    account.fetchDevices.mockResolvedValue([online("dev-a"), online("dev-b")]);
    await refreshDevices();
    account.fetchDevices.mockResolvedValue([online("dev-a")]);
    await refreshDevices();
    expect((await readCached(DEVICES_ADDRESS)).value.map((device) => device.id)).toEqual(["dev-a"]);
  });

  it("is not written by a read the account moved out from under", async () => {
    account.fetchDevices.mockResolvedValue([online("dev-a")]);
    await refreshDevices();
    account.fetchDevices.mockImplementation(async () => {
      App.accountEpoch += 1; // signed out while the read was in flight
      return [online("dev-stale")];
    });
    await expect(refreshDevices()).rejects.toThrow();
    expect((await readCached(DEVICES_ADDRESS)).value.map((device) => device.id)).toEqual(["dev-a"]);
  });

  it("commits modern policy and device rows together and rejects a replayed pull", async () => {
    const pending = Promise.withResolvers();
    const transport = { source: "hosted", rtc_allowed: false, reason: null, origin: "https://127.0.0.1:9443", assignment_revision: 2 };
    account.fetchDevices.mockReturnValueOnce(pending.promise).mockResolvedValueOnce({
      devices: [{ ...online("current"), transport }],
      transport_policy: { tunnel_access: true, promotion_revision: 3 },
    });
    const stale = refreshDevices();
    const rejected = expect(stale).rejects.toThrow("stale device presence read");
    await vi.waitFor(() => expect(account.fetchDevices).toHaveBeenCalledOnce());
    await refreshDevices();
    pending.resolve([online("old")]);
    await rejected;
    expect(App.devices).toEqual([{ ...online("current"), transport }]);
    expect(App.deviceTransportPolicy).toMatchObject({ policyAware: true, tunnelAccess: true, promotionRevision: 3 });
    await cacheTestState();
  });

  it("rejects an older pull when a different tab commits a higher order", async () => {
    setAccount(firstAccount);
    const requested = Promise.withResolvers();
    account.fetchDevices.mockReturnValueOnce(requested.promise);
    const pull = refreshDevices();
    const rejected = expect(pull).rejects.toThrow("stale device presence read");

    // The other tab shares the cache and its order counter, but it does not
    // increment this module's in-memory request generation.
    const order = await takeCachedCount({ ...DEVICES_ADDRESS, entityId: firstAccount, kind: "device-presence-order" });
    const policyAddress = { ...DEVICES_ADDRESS, kind: "device-transport-policy" };
    const newerDevice = hosted("dev-a", 9444, 2);
    const newerPolicy = { accountId: firstAccount, order, policyAware: true, tunnelAccess: true, promotionRevision: 3 };
    expect(await mergeCachedTogether([DEVICES_ADDRESS, policyAddress], () => [[newerDevice], newerPolicy])).toBe(true);
    expect(account.fetchDevices).toHaveBeenCalledOnce();

    requested.resolve({
      devices: [hosted("dev-a", 9443, 1)],
      transport_policy: { tunnel_access: true, promotion_revision: 2 },
    });
    await rejected;
    expect((await readCached(DEVICES_ADDRESS)).value).toEqual([newerDevice]);
    expect((await readCached(policyAddress)).value).toEqual(newerPolicy);
  });

  it("does not take up the previous account's listing before a cache wipe", async () => {
    setAccount(firstAccount);
    const row = hosted("dev-a", 9443, 1);
    account.fetchDevices.mockResolvedValueOnce({
      devices: [row], transport_policy: { tunnel_access: true, promotion_revision: 3 },
    });
    await refreshDevices();
    expect(App.deviceTransportPolicy?.accountId).toBe(firstAccount);

    setAccount(nextAccount);
    await expect(readCachedDevices({ offline: true })).resolves.toEqual([]);
    expect(App.devices).toEqual([]);
    expect(App.deviceTransportPolicy).toBeNull();
    expect((await readCached(DEVICES_ADDRESS)).value).toEqual([row]);
  });

  it("paints the picker from the cached list while the account pull is late", async () => {
    await writeCached(DEVICES_ADDRESS, [{ ...online("dev-cache"), name: "Cached laptop" }]);
    App.devices = [];
    account.fetchDevices.mockImplementation(() => new Promise(() => {}));

    initDevicePicker();
    void refreshDevices();

    await vi.waitFor(() => {
      expect(document.querySelector(".device-picker-toggle")?.textContent).toContain("All devices");
      expect(document.querySelector(".device-picker-menu")?.textContent).toContain("Cached laptop");
    });
  });

  it("redraws through a real cache write, announcement, and readback", async () => {
    initDevicePicker();
    await writeCached(DEVICES_ADDRESS, [{ ...online("dev-new"), name: "Announced desktop" }]);

    await vi.waitFor(() => {
      expect(App.devices.map((device) => device.id)).toEqual(["dev-new"]);
      expect(document.querySelector(".device-picker-menu")?.textContent).toContain("Announced desktop");
    });
  });
});
