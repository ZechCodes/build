// @vitest-environment jsdom
// One session per online device, each with its own offline state. A device that
// goes unreachable keeps its place — its rows stay in the merge, greyed, and the
// rest of the account carries on — so the banner only speaks when nothing at all
// is reachable.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const relay = vi.hoisted(() => ({ openRelaySession: vi.fn() }));
const terminals = vi.hoisted(() => ({ followTerminalDevice: vi.fn(), terminalDeviceId: vi.fn(() => null) }));

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
  followTerminalDevice: (...args) => terminals.followTerminalDevice(...args),
  terminalDeviceId: (...args) => terminals.terminalDeviceId(...args),
  // No terminal tab mounts in this suite; the socket and its status are still
  // answered, so a surface that asked for one would get a plain no.
  terminalManager: () => null,
  subscribeTerminalStatus: () => () => {},
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
const { contextFor, deviceFeedView, knownContexts, liveContexts } = await import(
  "../src/core/deviceContexts.js"
);
const { goOffline, openDeviceSessions, resume, setHomeDevice } = await import("../src/connection.js");
const { markDeviceOffline, markDeviceOnline } = await import("../src/devices.js");
const { startFeed, stopFeed, subscribeFeed } = await import("../src/core/taskFeed.js");
const { allDevicesOfflineText, offlineBannerText } = await import("../src/core/text.js");
const { mountInboxList } = await import("../src/core/inboxView.js");
const { initCompose, openCompose } = await import("../src/core/composeView.js");

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
      if (method === "project.list") return { projects: [{ project_id: "proj-1", name: `${deviceId} repo` }] };
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
    '<div id="root"></div><div id="devpick"></div><div id="offbar" hidden><span id="offbar-text"></span></div><div id="conn"></div><div id="compose"></div><div id="inbox-list"></div>';
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
  terminals.followTerminalDevice.mockClear();
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

/** Open the composer's manual panel, read the projects it offers, and close it
 *  again — the box takes its destinations from the home device's slice of the
 *  feed as that slice is delivered, which is what a home move has to move. */
function projectsOffered() {
  openCompose();
  document.querySelector("#compose-advanced").click();
  const names = [...document.querySelectorAll("#compose-project option")].map((option) => option.textContent);
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  return names;
}

const bannerText = () => document.getElementById("offbar-text").textContent;
const bannerShown = () => !document.getElementById("offbar").hidden;
const liveIds = () => liveContexts().map((context) => context.deviceId);

/** Boot: open every online device, as the gate does. It names no home — each
 *  device takes it in hand as it lands, if the account calls it home. */
async function connectEveryDevice() {
  const sessions = openDeviceSessions();
  await sessions.first;
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

  // The relay says a bridge went before that session is lost, so the account
  // lists it offline first — and with nothing else online there is no home left
  // to name. The aliases still describe that device, and what they have to say
  // about it now is that it is offline: the alternative is a composer that
  // thinks it can send and a view that never freezes.
  it("says the last device is offline, with no home left to name", async () => {
    await connectEveryDevice();
    unreachable.add("dev-a");
    unreachable.add("dev-b");

    markDeviceOffline("dev-b"); // the relay tells us each bridge went…
    goOffline("dev-b"); // …and each session is lost straight after
    markDeviceOffline("dev-a");
    goOffline("dev-a");
    await flush();

    expect(App.offline).toBe(true);
    expect(App.offlineSince).toBe(contextFor("dev-a").offlineSince);
    expect(bannerShown()).toBe(true);
  });

  // The banner's silence is only half the answer: the rail has to keep showing
  // the lost device's work, greyed, or its rows would simply vanish. The grey
  // is not news the feed carries — the rows themselves do not change when a
  // device goes — so the rail hears it from the registry and repaints at once,
  // rather than waiting out a poll that under push is 60 seconds long.
  it("keeps painting a lost device's rows, greyed the moment it goes", async () => {
    await connectEveryDevice();
    mountInboxList();
    startFeed(60000);
    await flush();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));
    expect(rowOn("dev-a")).toBeTruthy();

    unreachable.add("dev-a");
    goOffline("dev-a");
    await flush();

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);
    expect(rowOn("dev-b").classList.contains("inbox-offline")).toBe(false);
  });

  // And it comes back the same way: a device that answers again ungreys its
  // rows without waiting for one to move.
  it("ungreys a device's rows the moment it answers again", async () => {
    await connectEveryDevice();
    mountInboxList();
    startFeed(60000);
    await flush();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));

    unreachable.add("dev-a");
    goOffline("dev-a");
    await flush();
    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);

    unreachable.delete("dev-a");
    await resume("dev-a");
    await flush();

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(false);
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

  // A bridge that drops is heard twice: its own session is lost, and the relay
  // tells every other live session it went and again when it is back. The
  // resume already waiting on that device owns it — opening a second socket
  // costs another handshake, another greeting and a session for the bin.
  it("leaves a device that is already resuming to its resume", async () => {
    await connectEveryDevice();
    markDeviceOffline("dev-a");
    slowMs.set("dev-a", 1000); // the resume is still waiting on the relay
    goOffline("dev-a");
    await flush();
    const whileWaiting = openedFor("dev-a").length;

    markDeviceOnline("dev-a"); // the other session hears that bridge come back
    await flush();

    expect(openedFor("dev-a")).toHaveLength(whileWaiting);

    await vi.advanceTimersByTimeAsync(1000);
    expect(contextFor("dev-a").offline).toBe(false);
    expect(contextFor("dev-a").session).toBe(lastSession("dev-a"));
    expect(lastSession("dev-a").close).not.toHaveBeenCalled();
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

  // The pick is where creation goes and what the picker names, so a race
  // between two bridges must not decide it. The quicker device holds home only
  // until the picked one lands.
  it("hands home to the picked device even when another one answers first", async () => {
    App.selectedDeviceId = "dev-b";
    slowMs.set("dev-b", 20);

    const sessions = openDeviceSessions();
    await sessions.first; // the quicker device landed, and it is not the picked one
    expect(App.session).toBe(null); // nothing is home until the device the pick names answers

    await vi.advanceTimersByTimeAsync(20);
    await sessions.settled;
    await flush();

    expect(App.session).toBe(lastSession("dev-b"));
    expect(App.call).toBe(lastSession("dev-b").call);
    expect(App.cacheScope).toBe(contextFor("dev-b").cacheScope);
    expect(App.selectedDeviceId).toBe("dev-b"); // the pick itself is untouched
    expect(liveIds()).toEqual(["dev-a", "dev-b"]); // and nothing was closed
  });

  // The picker is not the only way home moves: the picked device landing after
  // another answered first moves it too, and so does a remembered device coming
  // back mid-session. The terminal socket reads the device it wants only when it
  // connects, and a healthy socket never reconnects on its own — so whichever
  // way home moved, it has to be dropped, or the shells keep typing at the
  // machine the user just left.
  it("moves the terminal socket with home, however home moved", async () => {
    App.selectedDeviceId = "dev-b";
    slowMs.set("dev-b", 20);
    const sessions = openDeviceSessions();
    await sessions.first; // the quicker device landed, and it is not the picked one
    terminals.followTerminalDevice.mockClear();

    await vi.advanceTimersByTimeAsync(20);
    await sessions.settled;
    await flush();

    expect(App.session).toBe(lastSession("dev-b"));
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
  });

  // Home is not a pointer anybody holds: it is what the account list and the
  // pick say. The relay saying the picked bridge went is news about home, so
  // everything that follows home — the aliases, the terminals, the picker, the
  // surfaces about here — moves to the device that can still answer.
  it("home falls back to the first online device when the picked one is marked offline by the relay", async () => {
    App.selectedDeviceId = "dev-b";
    await connectEveryDevice();
    expect(App.session).toBe(lastSession("dev-b"));
    terminals.followTerminalDevice.mockClear();

    unreachable.add("dev-b");
    markDeviceOffline("dev-b"); // another live session hears that bridge go
    await flush();

    expect(App.session).toBe(lastSession("dev-a"));
    expect(App.call).toBe(lastSession("dev-a").call);
    expect(App.offline).toBe(false); // the device home moved to is answering
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
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
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
    expect(captures.flush).toHaveBeenCalled(); // the new home takes what nobody could send
    expect(stayed.close).not.toHaveBeenCalled();
    expect(contextFor("dev-a").session).toBe(stayed);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // Home is what every surface about "here" is about: the composer's
  // destinations, the toolbar's projects, the capture decision page, the agent
  // rail. Each of them reads the home device's slice out of a snapshot as it
  // arrives and keeps it, so a home move that delivers nothing leaves them all
  // on the device the user just moved away from until some device's next tick —
  // a full minute while pushes are carrying the news.
  it("delivers a snapshot when home moves, so the surfaces about here follow", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    let here = null;
    const stop = subscribeFeed((snapshot) => (here = deviceFeedView(snapshot)));
    expect(here.items.map((item) => item.deviceId)).toEqual(["dev-a"]);

    await setHomeDevice("dev-b");

    expect(here.items.map((item) => item.deviceId)).toEqual(["dev-b"]);
    stop();
  });

  it("offers the new home device's projects the moment home moves", async () => {
    await connectEveryDevice();
    initCompose();
    startFeed(60000);
    await flush();
    expect(projectsOffered()).toEqual(["dev-a repo"]);

    await setHomeDevice("dev-b");

    expect(projectsOffered()).toEqual(["dev-b repo"]);
  });

  // Picking a device that has no session opens one, and the landing already
  // names it home — the pick is remembered before the connect. Taking it in
  // hand a second time on the way back re-points what is already pointed,
  // repaints the picker, delivers a second identical snapshot and offers the
  // capture queue twice.
  it("takes a newly opened home device in hand once, not twice", async () => {
    devices = [online("dev-a", "Laptop"), { id: "dev-b", name: "Desktop", status: "offline" }];
    App.devices = devices;
    await connectEveryDevice();
    App.devices = devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    captures.flush.mockClear();

    await setHomeDevice("dev-b");

    expect(App.session).toBe(lastSession("dev-b"));
    expect(captures.flush).toHaveBeenCalledTimes(1);
  });

  // A device that refused is kept after on a backoff. When it answers some
  // other way — the user picks it, or the relay says it is back and it joins —
  // that timer is still armed, and it fires at a device that is already live:
  // another handshake, another greeting, and a session for the bin.
  it("drops a resume still scheduled for a device that landed another way", async () => {
    unreachable.add("dev-b");
    await connectEveryDevice();
    unreachable.delete("dev-b");

    await setHomeDevice("dev-b");
    const landed = openedFor("dev-b").length;
    await vi.advanceTimersByTimeAsync(30000);

    expect(openedFor("dev-b")).toHaveLength(landed);
    expect(contextFor("dev-b").session).toBe(lastSession("dev-b"));
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
