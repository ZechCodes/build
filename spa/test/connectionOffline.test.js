// @vitest-environment jsdom
// One session per online device, each with its own offline state. A device that
// goes unreachable keeps its place — its rows stay in the merge, greyed, and the
// rest of the account carries on — so the banner only speaks when nothing at all
// is reachable.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const relay = vi.hoisted(() => ({ openRelaySession: vi.fn() }));
const terminals = vi.hoisted(() => ({ terminalsRideOn: vi.fn(), retargetTerminals: vi.fn() }));

let devices = [];

vi.mock("../src/core/session.js", () => ({
  openRelaySession: (options) => relay.openRelaySession(options),
}));
vi.mock("../src/api.js", () => ({
  fetchGatewayToken: async () => "tok",
  fetchIceServers: async () => [],
  fetchDevices: async () => devices,
}));
vi.mock("../src/terminal/manager.js", () => ({
  terminalsRideOn: (...args) => terminals.terminalsRideOn(...args),
  retargetTerminals: (...args) => terminals.retargetTerminals(...args),
}));
vi.mock("../src/core/peerLink.js", () => ({
  openPeerLink: async () => {
    throw new Error("no peer path in jsdom");
  },
}));
// The route render is not what this file is about; the shell still runs.
vi.mock("../src/views/inbox.js", () => ({ renderInbox: () => {} }));
// Everything the composer does is its own file's business; what matters here is
// which device is offered the captures nobody could send yet.
vi.mock("../src/core/composeView.js", async (importOriginal) => ({
  ...(await importOriginal()),
  flushCaptures: (...args) => captures.flush(...args),
}));
const captures = vi.hoisted(() => ({ flush: vi.fn(async () => {}) }));

const { App, disposeApplicationScope } = await import("../src/app.js");
const { contextFor, knownContexts, liveContexts } = await import("../src/core/deviceContexts.js");
const { claimHomeContext, goOffline, openDeviceSessions, resume, setHomeDevice } = await import(
  "../src/connection.js"
);
const { markDeviceOnline } = await import("../src/devices.js");
const { startFeed, stopFeed, subscribeFeed } = await import("../src/core/taskFeed.js");
const { allDevicesOfflineText, offlineBannerText } = await import("../src/core/text.js");
const { mountInboxList } = await import("../src/core/inboxView.js");

const flush = () => vi.advanceTimersByTimeAsync(0);

const online = (id, name) => ({ id, name, status: "online" });

const openedFor = (deviceId) => opened.filter((options) => options.preferDeviceId === deviceId);

let opened = [];
let unreachable = new Set();
let slowMs = new Map();
const handedOut = new Map(); // deviceId → the sessions that device was given, newest last

const lastSession = (deviceId) => (handedOut.get(deviceId) || []).at(-1);

/** A bridge session as the connection layer uses it: something to call, a
 *  carrier to hand over, and a way to say it is gone. */
function fakeSession(deviceId) {
  const session = {
    deviceId,
    call: vi.fn(async (method) => {
      if (method === "board.list") return { items: [{ id: `${deviceId}-row`, project_id: "proj-1", title: "Work" }] };
      if (method === "project.list") return { projects: [{ project_id: "proj-1", name: "Repo" }] };
      return {};
    }),
    peer: vi.fn(),
    onPush: () => () => {},
    onCarrier: vi.fn(),
    close: vi.fn(),
  };
  handedOut.set(deviceId, [...(handedOut.get(deviceId) || []), session]);
  return session;
}

let feed = null;
let unsubscribe = () => {};

beforeEach(() => {
  vi.useFakeTimers();
  disposeApplicationScope();
  stopFeed();
  document.body.innerHTML =
    '<div id="root"></div><div id="devpick"></div><div id="offbar" hidden><span id="offbar-text"></span></div><div id="conn"></div><div id="inbox-list"></div>';
  document.body.className = "";
  opened = [];
  unreachable = new Set();
  slowMs = new Map();
  handedOut.clear();
  feed = null;
  devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
  App.devices = devices;
  App.selectedDeviceId = null;
  App.route = { name: "inbox" };
  terminals.terminalsRideOn.mockClear();
  terminals.retargetTerminals.mockClear();
  captures.flush.mockClear();
  relay.openRelaySession.mockReset();
  relay.openRelaySession.mockImplementation(async (options) => {
    opened.push(options);
    const deviceId = options.preferDeviceId;
    if (unreachable.has(deviceId)) throw new Error(`${deviceId} is unreachable`);
    const wait = slowMs.get(deviceId);
    if (wait) await new Promise((done) => setTimeout(done, wait));
    return fakeSession(deviceId);
  });
  unsubscribe = subscribeFeed((snapshot) => (feed = snapshot));
});

afterEach(() => {
  unsubscribe();
  stopFeed();
  disposeApplicationScope();
  vi.clearAllTimers();
  vi.useRealTimers();
});

const bannerText = () => document.getElementById("offbar-text").textContent;
const bannerShown = () => !document.getElementById("offbar").hidden;
const liveIds = () => liveContexts().map((context) => context.deviceId);

/** Boot: open every online device and name the first one home, as the gate does. */
async function connectEveryDevice() {
  const sessions = openDeviceSessions();
  claimHomeContext(await sessions.first);
  const contexts = await sessions.settled;
  await flush();
  return contexts;
}

describe("per-device connections", () => {
  it("keeps a lost device known and offline while another device answers", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    unreachable.add("dev-a");

    goOffline("dev-a");
    await flush();

    expect(contextFor("dev-a").offline).toBe(true);
    expect(knownContexts().map((context) => context.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(liveIds()).toEqual(["dev-b"]);
    // Its rows are still the account's rows — greyed by the rail, not removed.
    expect(feed.items.map((item) => item.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(bannerShown()).toBe(false);
    expect(document.body.classList.contains("offline")).toBe(false);
    // dev-a is home, so the aliases the composer and the frozen views read must
    // say so — the banner's silence is about the account, not about them.
    expect(App.offline).toBe(true);
    expect(App.offlineSince).toBe(contextFor("dev-a").offlineSince);
  });

  // The banner's silence is only half the answer: the rail has to keep showing
  // the lost device's work, greyed, or its rows would simply vanish. The grey
  // arrives with the next paint, which is the next tick of a device that can
  // still answer — the rows themselves do not change when a device goes.
  it("keeps painting a lost device's rows, greyed", async () => {
    await connectEveryDevice();
    mountInboxList();
    startFeed(1000);
    await flush();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));
    expect(rowOn("dev-a")).toBeTruthy();

    unreachable.add("dev-a");
    goOffline("dev-a");
    await vi.advanceTimersByTimeAsync(1000);

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);
    expect(rowOn("dev-b").classList.contains("inbox-offline")).toBe(false);
  });

  it("says every device is offline once the last one goes", async () => {
    await connectEveryDevice();
    unreachable.add("dev-a");
    unreachable.add("dev-b");

    goOffline("dev-a");
    goOffline("dev-b");
    await flush();

    expect(bannerShown()).toBe(true);
    expect(bannerText()).toBe(allDevicesOfflineText());
    expect(document.body.classList.contains("offline")).toBe(true);
  });

  it("keeps naming the one device on the account, and when it went unreachable", async () => {
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;
    await connectEveryDevice();
    unreachable.add("dev-a");

    goOffline("dev-a");
    await flush();

    expect(bannerShown()).toBe(true);
    expect(bannerText()).toBe(offlineBannerText("Laptop", contextFor("dev-a").offlineSince));
  });

  it("reopens only the device resume names, waiting for it, and leaves the other session alone", async () => {
    await connectEveryDevice();
    const lost = lastSession("dev-a");
    const other = lastSession("dev-b");
    unreachable.add("dev-a");
    goOffline("dev-a");
    await flush();
    unreachable.delete("dev-a");

    await resume("dev-a");
    await flush();

    const reopened = openedFor("dev-a").at(-1);
    expect(reopened.preferDeviceId).toBe("dev-a");
    expect(reopened.waitForDevice).toBe(true);
    expect(contextFor("dev-a").offline).toBe(false);
    expect(contextFor("dev-a").session).not.toBe(lost);
    // Nothing was asked of the device that never left.
    expect(openedFor("dev-b")).toHaveLength(1);
    expect(contextFor("dev-b").session).toBe(other);
    expect(other.close).not.toHaveBeenCalled();
    expect(App.offline).toBe(false); // home is back, and the aliases say so
  });

  it("hands back a resumed session for a device that came back another way", async () => {
    await connectEveryDevice();
    slowMs.set("dev-a", 1000);
    goOffline("dev-a"); // the device's own resume starts waiting for it
    await flush();
    slowMs.delete("dev-a");

    await setHomeDevice("dev-a"); // a connect that does not wait gets there first
    const live = lastSession("dev-a");
    await vi.advanceTimersByTimeAsync(1000);

    const late = lastSession("dev-a");
    expect(late).not.toBe(live);
    expect(late.close).toHaveBeenCalled(); // nothing needs it: the device is live
    expect(contextFor("dev-a").session).toBe(live);
    expect(contextFor("dev-a").offline).toBe(false);
  });

  it("connects a device that comes online after boot, without a reload", async () => {
    devices = [online("dev-a", "Laptop"), { id: "dev-b", name: "Desktop", status: "offline" }];
    App.devices = devices;
    await connectEveryDevice();
    expect(liveIds()).toEqual(["dev-a"]);
    captures.flush.mockClear(); // home already took the queue when it came up

    markDeviceOnline("dev-b");
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
    expect(contextFor("dev-b").session).toBe(lastSession("dev-b"));
    // It joined the account, it did not take it over: home is untouched, and
    // the captures waiting for a device are not offered to it.
    expect(App.session).toBe(lastSession("dev-a"));
    expect(captures.flush).not.toHaveBeenCalled();
  });

  // Paired in another tab: the relay pushes that device's key before this tab's
  // list has ever heard of it. Reading the list is only half of joining it —
  // without the open it shows in the picker as online and contributes no rows
  // until it reconnects.
  it("connects a device paired in another tab, once the list has caught up", async () => {
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;
    await connectEveryDevice();
    expect(liveIds()).toEqual(["dev-a"]);

    devices = [online("dev-a", "Laptop"), online("dev-c", "Studio")];
    markDeviceOnline("dev-c");
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-c"]);
    expect(contextFor("dev-c").session).toBe(lastSession("dev-c"));
  });

  it("resolves the first success without waiting on the slowest device", async () => {
    slowMs.set("dev-b", 5000);

    const sessions = openDeviceSessions();
    const first = await sessions.first;

    expect(first.deviceId).toBe("dev-a");
    expect(contextFor("dev-b")).toBe(null); // still connecting: nothing waited for it

    await vi.advanceTimersByTimeAsync(5000);
    const contexts = await sessions.settled;
    expect(contexts.map((context) => context.deviceId)).toEqual(["dev-a", "dev-b"]);
  });

  it("re-points the aliases and the terminals on a new home device, and closes nothing", async () => {
    await connectEveryDevice();
    const stayed = lastSession("dev-a");
    const home = lastSession("dev-b");

    await setHomeDevice("dev-b");

    expect(App.selectedDeviceId).toBe("dev-b");
    expect(App.session).toBe(home);
    expect(App.call).toBe(home.call);
    expect(App.cacheScope).toBe(contextFor("dev-b").cacheScope);
    expect(terminals.retargetTerminals).toHaveBeenCalled();
    expect(captures.flush).toHaveBeenCalled(); // the new home takes what nobody could send
    expect(stayed.close).not.toHaveBeenCalled();
    expect(contextFor("dev-a").session).toBe(stayed);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  it("pauses only the calls of the device that went offline", async () => {
    await connectEveryDevice();
    const lost = openedFor("dev-a").at(-1);
    const kept = openedFor("dev-b").at(-1);
    unreachable.add("dev-a");

    lost.onLost(); // the session says it is gone, the way relayLink does
    await flush();

    expect(lost.isPaused()).toBe(true);
    expect(kept.isPaused()).toBe(false);
    expect(contextFor("dev-b").offline).toBe(false);
  });
});
