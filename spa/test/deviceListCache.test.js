// @vitest-environment jsdom
// The account's device list is the one thing in the cache that is nobody's
// device: it comes from `GET /api/devices` on skriftapp, not from a bridge.
// The presence poll is the only reader of it, so the poll is what writes it —
// and it is written so a reload paints the rail's machines before the first
// read of the account has answered.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const account = vi.hoisted(() => ({ fetchDevices: vi.fn() }));

vi.mock("../src/api.js", () => ({ fetchDevices: (...args) => account.fetchDevices(...args) }));
vi.mock("../src/connection.js", () => ({
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
const { refreshDevices } = await import("../src/devices.js");
// The one address, read from where both sides of it read it: the writer here
// and the boot paint (views/gate.js) agree on it because there is one
// constant, and a test that restated it would stay green while they drifted.
const { DEVICES_ADDRESS, readCached } = await import("../src/core/localCache.js");

const online = (id) => ({ id, name: id, status: "online", fingerprint: `${id}-fp` });

beforeEach(() => {
  document.body.innerHTML = '<div id="devpick"></div>';
  App.devices = [];
  App.accountEpoch = 0;
  account.fetchDevices.mockReset();
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
});
