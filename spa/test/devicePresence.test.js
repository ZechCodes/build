// @vitest-environment jsdom
// Presence is the api's (spec rule 6). The relay says nothing about which
// machines are up any more: `GET /api/devices` is the only answer, and this
// poll is what re-reads it — every 15 s while the app is open, and at once when
// the tab comes back to the front.
//
// Three things follow every read: a machine this client holds that the account
// no longer has at all is retired, a machine that is online and has no live
// session is opened (that is how a late device joins, and how one that came
// back is taken up again), and a machine this client holds that the account no
// longer lists online is marked away.

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../src/core/localCache.js", () => import("./memoryCache.js"));
vi.mock("../src/core/localUiStore.js", () => import("./memoryUiStore.js"));

const connection = vi.hoisted(() => ({
  openDeviceSessions: vi.fn(() => ({ first: Promise.resolve(null), settled: Promise.resolve([]) })),
  deviceWentAway: vi.fn(),
  retireDevice: vi.fn(),
  syncHome: vi.fn(),
  syncDeviceRecoveryPresence: vi.fn(),
}));
const account = vi.hoisted(() => ({ fetchDevices: vi.fn() }));

vi.mock("../src/api.js", () => ({ fetchDevices: (...args) => account.fetchDevices(...args) }));
vi.mock("../src/connection.js", () => ({
  deviceRecoverySnapshot: () => [], onDeviceRecoveryChanged: () => () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {},
  openDeviceSessions: (...args) => connection.openDeviceSessions(...args),
  deviceWentAway: (...args) => connection.deviceWentAway(...args),
  syncHome: (...args) => connection.syncHome(...args),
  syncDeviceRecoveryPresence: (...args) => connection.syncDeviceRecoveryPresence(...args),
  chooseCreationDevice: () => {},
  connectDevice: () => Promise.resolve(null),
  goOffline: () => {},
  retireDevice: (...args) => connection.retireDevice(...args),
  openDeviceSettingsSession: async () => ({}),
  forgetHomeFollow: () => {},
  forgetRendezvousSockets: () => {},
  forgetSecurityStops: () => {},
  securityStopText: () => "",
}));

const { App } = await import("../src/app.js");
const { refreshDevices, watchPresence, stopWatchingPresence } = await import("../src/devices.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { fakeSession } = await import("./deviceSessionFixture.js");
const { clearMemoryCacheRecords } = await import("./memoryCache.js");

const online = (id) => ({ id, name: id, status: "online", fingerprint: `${id}-fp` });
const away = (id) => ({ ...online(id), status: "offline" });

let listed = [];

beforeEach(() => {
  vi.useFakeTimers();
  clearMemoryCacheRecords();
  document.body.innerHTML = '<div id="devpick"></div>';
  resetDeviceContexts();
  App.devices = [];
  App.gated = false;
  listed = [online("dev-a"), online("dev-b")];
  account.fetchDevices.mockReset();
  account.fetchDevices.mockImplementation(async () => listed);
  connection.openDeviceSessions.mockClear();
  connection.deviceWentAway.mockClear();
  connection.retireDevice.mockClear();
  connection.syncHome.mockClear();
  connection.syncDeviceRecoveryPresence.mockClear();
});

afterEach(() => {
  stopWatchingPresence();
  resetDeviceContexts();
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("the presence poll", () => {
  it("re-reads the account on its own cadence while the app is open", async () => {
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);
    expect(account.fetchDevices).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(30000);
    expect(account.fetchDevices).toHaveBeenCalledTimes(3);
    expect(App.devices.map((device) => device.id)).toEqual(["dev-a", "dev-b"]);
  });

  it("reads it at once when the tab comes back to the front", async () => {
    watchPresence();

    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);

    expect(account.fetchDevices).toHaveBeenCalledTimes(1);
  });

  it("reads nothing once it is stopped — the gate took the app back, or the account did", async () => {
    watchPresence();
    stopWatchingPresence();

    await vi.advanceTimersByTimeAsync(60000);
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(0);

    expect(account.fetchDevices).not.toHaveBeenCalled();
  });

  it("watches once however often it is asked to", async () => {
    watchPresence();
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(account.fetchDevices).toHaveBeenCalledTimes(1);
  });

  // The late join: a machine that came online after boot, or was paired in
  // another tab, is opened without a reload.
  it("opens every online device with no live session after each read", async () => {
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(connection.openDeviceSessions).toHaveBeenCalledTimes(1);
    expect(connection.syncHome).toHaveBeenCalled();
  });

  it("keeps the last known presence when the device read fails", async () => {
    App.devices = [online("dev-a")];
    account.fetchDevices.mockRejectedValueOnce(new Error("api unavailable"));

    const result = await (await import("../src/devices.js")).readPresence();

    expect(result).toBeNull();
    expect(App.devices).toEqual([online("dev-a")]);
    expect(connection.syncDeviceRecoveryPresence).not.toHaveBeenCalled();
  });

  it("ignores a presence response that lands after watching stops", async () => {
    App.devices = [online("dev-a")];
    let resolveRead;
    account.fetchDevices.mockImplementationOnce(() => new Promise((resolve) => { resolveRead = resolve; }));
    const pending = (await import("../src/devices.js")).readPresence();
    stopWatchingPresence();

    resolveRead([away("dev-a")]);
    await expect(pending).resolves.toBeNull();

    expect(App.devices).toEqual([online("dev-a")]);
    expect(connection.syncDeviceRecoveryPresence).not.toHaveBeenCalled();
    expect(connection.openDeviceSessions).not.toHaveBeenCalled();
    expect(connection.deviceWentAway).not.toHaveBeenCalled();
  });

  // The other half of rule 6: the bridge stopped posting its heartbeat, so the
  // api stopped calling it online, so the machine this client is holding is
  // away. That is the slow signal — a live connection failing is the fast one.
  it("marks a machine away when the account stops calling it online", async () => {
    adoptDeviceSession(fakeSession("dev-a"));
    adoptDeviceSession(fakeSession("dev-b"));
    listed = [online("dev-a"), away("dev-b")];
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(connection.deviceWentAway.mock.calls).toEqual([["dev-b"]]);
  });

  // Listed offline is away; not listed at all is gone (#141). The account let
  // the machine go, so it is retired as an explicit removal retires it.
  it("retires a machine the account stops listing at all, rather than marking it away", async () => {
    adoptDeviceSession(fakeSession("dev-a"));
    adoptDeviceSession(fakeSession("dev-c"));
    listed = [online("dev-a")];
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(connection.retireDevice.mock.calls).toEqual([["dev-c"]]);
    expect(connection.deviceWentAway).not.toHaveBeenCalled();
  });

  // A machine this client learned of while the read was in flight — paired in
  // another tab, which wrote the list — may be newer than the answer, which
  // was asked before anything here knew of it. The next read decides.
  it("does not retire a machine that turned up while the read was in flight", async () => {
    let answer;
    account.fetchDevices.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
    const pending = (await import("../src/devices.js")).readPresence();
    adoptDeviceSession(fakeSession("dev-c"));

    answer([online("dev-a")]);
    await pending;

    expect(connection.retireDevice).not.toHaveBeenCalled();
  });

  // Another tab's write is a projection of disk, not a read of the account:
  // a list that lands over this one still naming the machine keeps it.
  it("does not retire a machine the committed list still names", async () => {
    const { DEVICES_ADDRESS, subscribeCache, writeCached } = await import("../src/core/localCache.js");
    adoptDeviceSession(fakeSession("dev-c"));
    listed = [online("dev-a")];
    const stop = subscribeCache(DEVICES_ADDRESS, () => {
      stop(); // this read's own write, and the other tab's lands right after it
      void writeCached(DEVICES_ADDRESS, [online("dev-a"), online("dev-c")]);
    });
    const { readPresence } = await import("../src/devices.js");

    await readPresence();

    expect(App.devices.map((device) => device.id)).toContain("dev-c");
    expect(connection.retireDevice).not.toHaveBeenCalled();
  });

  it("retires nothing on a read that fails or is superseded", async () => {
    adoptDeviceSession(fakeSession("dev-c"));
    const { readPresence } = await import("../src/devices.js");
    account.fetchDevices.mockRejectedValueOnce(new Error("api unavailable"));
    await readPresence();

    let answer;
    account.fetchDevices.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
    const pending = readPresence();
    stopWatchingPresence();
    answer([]);
    await pending;

    expect(connection.retireDevice).not.toHaveBeenCalled();
  });

  it("says nothing about a machine the account still calls online", async () => {
    adoptDeviceSession(fakeSession("dev-a"));
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(connection.deviceWentAway).not.toHaveBeenCalled();
  });

  // The api is not always there either. A read that fails is not news about any
  // machine — marking every device away on a flaky network would empty the app.
  it("leaves every machine exactly as it was when the account cannot be read", async () => {
    adoptDeviceSession(fakeSession("dev-a"));
    account.fetchDevices.mockRejectedValue(new Error("offline"));
    watchPresence();

    await vi.advanceTimersByTimeAsync(15000);

    expect(connection.deviceWentAway).not.toHaveBeenCalled();
    expect(connection.openDeviceSessions).not.toHaveBeenCalled();
  });

  // The account list is read by other things too — the gate's own 3 s poll, the
  // settings page. Reading it is not what opens or marks anything; the poll is.
  it("is the reader that acts on a refresh, not refreshDevices itself", async () => {
    adoptDeviceSession(fakeSession("dev-a"));
    listed = [away("dev-a")];

    await refreshDevices();

    expect(connection.deviceWentAway).not.toHaveBeenCalled();
    expect(connection.openDeviceSessions).not.toHaveBeenCalled();
  });

  it("retires nothing on a refresh either: the account list is read, not acted on", async () => {
    adoptDeviceSession(fakeSession("dev-c"));
    listed = [];

    await refreshDevices();

    expect(connection.retireDevice).not.toHaveBeenCalled();
  });
});
