// @vitest-environment jsdom
// One session per device, each live over that device's own direct connection
// and blocked on its own when that connection cannot be made (spec rule 3). A
// device that goes keeps its place — its rows stay in the merge, greyed, and
// the rest of the account carries on — so the account-wide screen only speaks
// when nothing at all can answer.
//
// What is stood in for here is the wire: the rendezvous, the session it mints
// and the peer link it negotiates. What is real is the policy over them — which
// machines are opened, what a failure costs that machine, where home is, and
// when the gate takes the app back.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const terminals = vi.hoisted(() => ({ followTerminalDevice: vi.fn(), terminalDeviceId: vi.fn(() => null) }));
const wire = vi.hoisted(() => ({ openSession: vi.fn(), openPeerLink: vi.fn() }));

let devices = [];

vi.mock("../src/core/rendezvous.js", () => ({
  createRelayRendezvous: (options) => makeRendezvous(options),
}));
vi.mock("../src/core/session.js", () => ({
  openSession: (options) => wire.openSession(options),
}));
vi.mock("../src/core/peerLink.js", () => ({ openPeerLink: (options) => wire.openPeerLink(options) }));
const account = vi.hoisted(() => ({ fetchDevices: null }));
vi.mock("../src/api.js", () => ({
  fetchGatewayToken: async () => "tok",
  fetchIceServers: async () => [],
  fetchDevices: (...args) => account.fetchDevices(...args),
}));
vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice: (...args) => terminals.followTerminalDevice(...args),
  terminalDeviceId: (...args) => terminals.terminalDeviceId(...args),
  provideTerminalSessions: (mint) => {
    mints.provided = mint;
  },
  // No terminal tab mounts in this suite; the socket and its status are still
  // answered, so a surface that asked for one would get a plain no.
  terminalManager: () => null,
  subscribeTerminalStatus: () => () => {},
}));
// The route render is not what this file is about; the shell still runs, and
// the gate handing the app back is one of the things this file is about.
const routes = vi.hoisted(() => ({ renderInbox: vi.fn() }));
vi.mock("../src/views/inbox.js", () => ({ renderInbox: (...args) => routes.renderInbox(...args) }));
// Everything the composer does is its own file's business; what matters here is
// which device is offered the captures nobody could send yet.
vi.mock("../src/core/composeView.js", async (importOriginal) => ({
  ...(await importOriginal()),
  flushCaptures: (...args) => captures.flush(...args),
}));
const captures = vi.hoisted(() => ({ flush: vi.fn(async () => {}) }));
const mints = vi.hoisted(() => ({ provided: null }));

const { App, resetApplication, rememberSelectedDevice } = await import("../src/app.js");
const { contextFor, deviceFeedView, homeContext, knownContexts, liveContexts } = await import(
  "../src/core/deviceContexts.js"
);
const {
  chooseCreationDevice,
  connectDevice,
  deviceWentAway,
  goOffline,
  openDeviceSessions,
  retireDevice,
  syncHome,
} = await import("../src/connection.js");
const { initDevicePicker, paintDevicePicker, stopWatchingPresence } = await import("../src/devices.js");
const { startFeed, stopFeed, subscribeFeed } = await import("../src/core/taskFeed.js");
const { allDevicesOfflineText, deviceUnreachableText } = await import("../src/core/text.js");
const { mountInboxList } = await import("../src/core/inboxView.js");
const { initCompose, openCompose } = await import("../src/core/composeView.js");
const { holdAppWhileNoDeviceAnswers } = await import("../src/views/gate.js");

const flush = () => vi.advanceTimersByTimeAsync(0);

const online = (id, name) => ({ id, name, status: "online", fingerprint: `${id}-fingerprint` });
/** The same paired device, as the account lists it while its bridge is down. */
const away = (id, name) => ({ ...online(id, name), status: "offline" });

/** Every time this machine was asked for a session. */
const openedFor = (deviceId) => opened.filter((options) => options.deviceId === deviceId);

/** How many times this machine's bridge has been asked for its board. */
const boardReads = (deviceId) =>
  (lastSession(deviceId)?.call.mock.calls || []).filter(([method]) => method === "board.list").length;

let opened = [];
let unreachable = new Set(); // machines whose rendezvous will not find their bridge
let unlinkable = new Set(); // machines whose direct connection will not open
// Machines offering a key that is not the key this account pinned.
let impostors = new Set();
let slowMs = new Map();
const handedOut = new Map(); // deviceId → the sessions that device was given, newest last
const rendezvousFor = new Map(); // deviceId → the one rendezvous this layer made for it
const linksFor = new Map(); // deviceId → the direct connection it is riding

const lastSession = (deviceId) => (handedOut.get(deviceId) || []).at(-1);

// What a machine's bridge does when it is greeted, where a case wants to say:
// answer when the test lets it, or refuse `session.hello` the way a bridge that
// predates it does. Unnamed machines answer at once.
const greetings = new Map(); // deviceId → () => Promise

/** One machine's rendezvous, as the connection layer holds it: minting is the
 *  whole of what this file asks of it, and whether it is open. */
function makeRendezvous({ deviceId }) {
  const rendezvous = {
    deviceId,
    open: vi.fn(async () => {}),
    mint: vi.fn(async () => ({ sessionId: `sess-${deviceId}`, sessionKeyB64: "key", deviceId })),
    signalCarrier: vi.fn(() => ({ onClose: vi.fn(), close: vi.fn() })),
    isOpen: () => rendezvous.opened,
    onClosed: () => () => {},
    opened: true,
    close: vi.fn(() => {
      rendezvous.opened = false;
    }),
  };
  rendezvousFor.set(deviceId, rendezvous);
  return rendezvous;
}

/** A direct connection as the connection layer uses it: two channels that can
 *  say they closed, and a way to close the pair. */
function fakePeerLink(deviceId) {
  const carrier = () => {
    const listeners = new Set();
    return { onClose: (fn) => listeners.add(fn), drop: () => listeners.forEach((fn) => fn()) };
  };
  const link = { app: carrier(), term: carrier(), close: vi.fn() };
  linksFor.set(deviceId, link);
  return link;
}

/** A bridge session as the connection layer uses it: something to call, a
 *  channel to ride, and a way to say it is gone. Taken off its channel — by the
 *  channel closing, or by the layer handing it back — it reports the loss
 *  exactly as the real session does: nothing else ever carried it. */
function fakeSession(deviceId, onLost = () => {}) {
  const session = {
    deviceId,
    call: vi.fn(async (method) => {
      if (method === "session.hello" && greetings.has(deviceId)) return greetings.get(deviceId)();
      if (method === "board.list") return { items: [{ id: `${deviceId}-row`, project_id: "proj-1", title: "Work" }] };
      if (method === "project.list") return { projects: [{ project_id: "proj-1", name: `${deviceId} repo` }] };
      // The rail lists workspaces, so a machine with none paints no rows at
      // all: each one keeps a checkout of the project both machines name.
      if (method === "workspace.list")
        return { workspaces: [{ workspace_id: `${deviceId}-workspace`, project_id: "proj-1", title: "Work", state: "running" }] };
      return { deviceId };
    }),
    peer: vi.fn((carrier) => {
      if (!carrier && !session.closed) onLost();
    }),
    fail: vi.fn(),
    onPush: () => () => {},
    onCarrier: vi.fn(),
    reattachSignaling: vi.fn(async () => {}),
    closed: false,
    close: vi.fn(() => {
      session.closed = true; // a session its client closed reports nothing more
    }),
  };
  handedOut.set(deviceId, [...(handedOut.get(deviceId) || []), session]);
  return session;
}

let feed = null;
let unsubscribe = () => {};

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  resetApplication();
  stopFeed();
  document.body.innerHTML =
    '<div id="root"></div><div id="devpick"></div><div id="compose"></div><div id="inbox-list"></div>';
  document.body.className = "";
  App.gated = false;
  App.poll = null;
  App.viewDispose = null;
  globalThis.RTCPeerConnection = function RTCPeerConnectionStub() {};
  routes.renderInbox.mockClear();
  // The app is entered: the gate is listening for the account running out of
  // machines to answer, which is what holds it and what hands it back.
  holdAppWhileNoDeviceAnswers();
  opened = [];
  unreachable = new Set();
  unlinkable = new Set();
  impostors = new Set();
  slowMs = new Map();
  greetings.clear();
  handedOut.clear();
  rendezvousFor.clear();
  linksFor.clear();
  feed = null;
  devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
  App.devices = devices;
  App.selectedDeviceId = null;
  App.route = { name: "inbox" };
  terminals.followTerminalDevice.mockClear();
  captures.flush.mockClear();
  account.fetchDevices = vi.fn(async () => devices);
  wire.openSession.mockReset();
  wire.openSession.mockImplementation(async (options) => {
    opened.push(options);
    const deviceId = options.deviceId;
    if (impostors.has(deviceId))
      throw Object.assign(new Error("no pinned transport key for device — refusing to open a session"), {
        securityCritical: true,
      });
    if (unreachable.has(deviceId)) throw new Error("device did not answer");
    const wait = slowMs.get(deviceId);
    if (wait) await new Promise((done) => setTimeout(done, wait));
    return fakeSession(deviceId, options.onLost);
  });
  wire.openPeerLink.mockReset();
  wire.openPeerLink.mockImplementation(async ({ signal, onConnected }) => {
    const { deviceId } = await signal("rtc.offer", { sdp: "v=0" });
    if (unlinkable.has(deviceId)) throw Object.assign(new Error("no channels"), { blockedReason: "timeout" });
    await onConnected();
    return fakePeerLink(deviceId);
  });
  unsubscribe = subscribeFeed((snapshot) => (feed = snapshot));
});

afterEach(() => {
  delete globalThis.RTCPeerConnection;
  stopWatchingPresence();
  unsubscribe();
  stopFeed();
  resetApplication();
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

/** Whether the gate is holding the app: nothing can answer, so there is no
 *  route to stand on and the waiting screen owns the page. */
const held = () => document.body.classList.contains("gated");
const waitingNote = () => document.getElementById("waitintro")?.textContent || "";
const waitingHeading = () => document.querySelector("#root h1")?.textContent || "";
const liveIds = () => liveContexts().map((context) => context.deviceId);

/** Boot: open every online device, as the gate does. It names no home — each
 *  device takes it in hand as it lands, if the account calls it home. */
async function connectEveryDevice() {
  const sessions = openDeviceSessions();
  await sessions.first.catch(() => {});
  const contexts = await sessions.settled;
  await flush();
  return contexts;
}

/** A live machine whose direct connection goes: the channels close, which is
 *  the only signal this layer ever had that a live machine has gone. */
async function loseTheLink(deviceId) {
  linksFor.get(deviceId).app.drop();
  await flush();
}

describe("per-device connections", () => {
  // The greeting is what says which API major this bridge speaks. A read issued
  // before it lands is a read whose answer this tab may not be able to make
  // sense of — and on a bridge the greeting then calls unsupported, it is a
  // machine that has already been asked something it will not be asked again.
  // So the feed's first read of a machine waits for that machine's greeting,
  // and for no other machine's.
  it("asks a machine for nothing until its own greeting has settled", async () => {
    let greet;
    greetings.set("dev-a", () => new Promise((settle) => { greet = settle; }));
    openDeviceSessions();
    await flush();
    startFeed(60000);
    await flush();

    expect(boardReads("dev-a")).toBe(0);
    expect(boardReads("dev-b")).toBeGreaterThan(0); // the other machine is not held up

    greet({});
    await flush();

    expect(boardReads("dev-a")).toBeGreaterThan(0);
  });

  // A bridge that predates `session.hello` refuses the verb, and that refusal is
  // the whole of the feature detection: it has answered. Waiting on a greeting
  // must not be waiting on a greeting that can never arrive.
  it("reads a machine whose bridge refuses the greeting", async () => {
    greetings.set("dev-a", async () => {
      throw new Error("unknown method: session.hello");
    });

    await connectEveryDevice();
    startFeed(60000);
    await flush();

    expect(boardReads("dev-a")).toBeGreaterThan(0);
  });

  it("gives every machine one rendezvous, and closes it once the channels carry", async () => {
    await connectEveryDevice();

    expect([...rendezvousFor.keys()]).toEqual(["dev-a", "dev-b"]);
    for (const rendezvous of rendezvousFor.values()) expect(rendezvous.close).toHaveBeenCalled();
    // And the terminals' mint is this layer's, on the same rendezvous.
    expect(mints.provided).toBeTypeOf("function");
    await mints.provided("dev-a");
    expect(rendezvousFor.get("dev-a").mint).toHaveBeenCalledTimes(1);
  });

  it("keeps a lost device known and blocked while another device answers", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();

    await loseTheLink("dev-a");

    expect(contextFor("dev-a").offline).toBe(true);
    expect(contextFor("dev-a").blocked).toBe("lost");
    expect(knownContexts().map((context) => context.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(liveIds()).toEqual(["dev-b"]);
    // Its rows are still the account's rows — greyed by the rail, not removed.
    expect(feed.items.map((item) => item.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(held()).toBe(false); // the account still has a machine to stand on
    // The account still lists dev-a online, so it is still where creation would
    // go — and what the composer reads off it is that it cannot answer.
    expect(homeContext()).toBe(contextFor("dev-a"));
    expect(homeContext().offline).toBe(true);
  });

  // Rule 3 is per device and nothing else: a machine nothing could reach is
  // blocked, and the account carries on over the ones that answered.
  it("does not hold the app for a blocked device while another one is live", async () => {
    unlinkable.add("dev-b");

    await connectEveryDevice();

    expect(liveIds()).toEqual(["dev-a"]);
    expect(contextFor("dev-b").blocked).toBe("timeout");
    expect(held()).toBe(false);
  });

  // The banner's silence is only half the answer: the rail has to keep showing
  // the lost device's work, greyed, or its rows would simply vanish. The grey
  // is not news the feed carries — the rows themselves do not change when a
  // device goes — so the rail hears it from the registry and repaints at once.
  it("keeps painting a lost device's rows, greyed the moment it goes", async () => {
    await connectEveryDevice();
    mountInboxList();
    startFeed(60000);
    await flush();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));
    expect(rowOn("dev-a")).toBeTruthy();

    await loseTheLink("dev-a");

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);
    expect(rowOn("dev-b").classList.contains("inbox-offline")).toBe(false);
  });

  // And it comes back the same way: a device that answers again ungreys its
  // rows without waiting for anything to move.
  it("ungreys a device's rows the moment a retry lands it", async () => {
    await connectEveryDevice();
    mountInboxList();
    startFeed(60000);
    await flush();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));

    await loseTheLink("dev-a");
    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);

    await connectDevice("dev-a");
    await flush();

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(false);
    expect(contextFor("dev-a").blocked).toBe(null);
  });

  // A machine that could not be reached is not asked for again on a cadence:
  // there is nothing under it to fall back to and nothing that would change
  // between one try and the next (rule 3).
  it("does not dial a blocked machine again until a reader or the account asks", async () => {
    unlinkable.add("dev-b");
    await connectEveryDevice();
    const dialled = openedFor("dev-b").length;

    await openDeviceSessions().settled;
    await vi.advanceTimersByTimeAsync(60000);

    expect(openedFor("dev-b")).toHaveLength(dialled);

    unlinkable.delete("dev-b");
    await connectDevice("dev-b");
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  it("says every device is offline once the last one goes", async () => {
    await connectEveryDevice();

    await loseTheLink("dev-a");
    await loseTheLink("dev-b");

    expect(held()).toBe(true);
    expect(waitingNote()).toContain(allDevicesOfflineText());
  });

  // Which sentence this is, is the account's question rather than this client's.
  // A machine that was already down at boot was never opened here and has no
  // context, so counting contexts called an account of two machines an account
  // of one — and named whichever one this client happened to hold.
  it("says every device is offline when one of them was already down at boot", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();

    await loseTheLink("dev-a");

    expect(held()).toBe(true);
    expect(waitingNote()).toContain(allDevicesOfflineText());
  });

  it("keeps naming the one device on the account, and when it went unreachable", async () => {
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;
    await connectEveryDevice();

    await loseTheLink("dev-a");

    expect(held()).toBe(true);
    expect(waitingHeading()).toBe("Waiting for your device");
    expect(waitingNote()).toContain(deviceUnreachableText("Laptop", contextFor("dev-a").offlineSince));
  });

  // Every surface is about a machine, so an account with none has nothing to
  // stand on: the gate takes the app back rather than leaving a dead route
  // under a banner. The view goes with it — its poll and its own teardown.
  it("holds the app on the waiting screen when the last device goes", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    const poll = { dispose: vi.fn() };
    const viewDispose = vi.fn();
    App.poll = poll;
    App.viewDispose = viewDispose;

    await loseTheLink("dev-a");
    await loseTheLink("dev-b");

    expect(held()).toBe(true);
    // Two machines are listed under it, and either of them hands the app back.
    expect(waitingHeading()).toBe("Waiting for a device");
    expect(document.getElementById("waitlist").textContent).toContain("Laptop");
    expect(document.getElementById("waitlist").textContent).toContain("Desktop");
    expect(poll.dispose).toHaveBeenCalled();
    expect(viewDispose).toHaveBeenCalled();
    expect(App.poll).toBe(null);
    expect(App.viewDispose).toBe(null);
  });

  // The screen's foot is one line about what the app is doing and the two things
  // a reader can do about it.
  it("lays the waiting screen's foot out as one row", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(held()).toBe(true);

    const foot = document.getElementById("watchmsg").parentElement;
    expect(foot.className.split(/\s+/)).toContain("wait-row");
    expect([...foot.children].map((child) => child.id)).toEqual(["watchmsg", "retrybtn", "addmore"]);
    // Where each of them sits is the sheet's to say, not the markup's.
    expect(document.getElementById("retrybtn").getAttribute("style")).toBe(null);
  });

  // "Retry now" starts the waiting screen polling for a device. A machine that
  // comes back some other way — a retry landing through the hold listener —
  // hands the app straight back, and the poll left armed would re-enter the app
  // over a reader already standing in it.
  it("stops the waiting screen's poll when a device lands another way", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(held()).toBe(true);

    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    document.getElementById("retrybtn").click();
    await flush();
    routes.renderInbox.mockClear();

    // The bridge comes back, and a retry on that machine is what lands it.
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectDevice("dev-a");
    await flush();
    expect(held()).toBe(false);
    expect(routes.renderInbox).toHaveBeenCalledTimes(1);
    account.fetchDevices.mockClear();

    await vi.advanceTimersByTimeAsync(7000);

    expect(account.fetchDevices).not.toHaveBeenCalled();
    expect(routes.renderInbox).toHaveBeenCalledTimes(1);
  });

  // And the way back is the same signal: the machine that answers hands the
  // reader their route back, with the feed reading it again — no reload.
  it("hands the app back the moment one device answers again", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(held()).toBe(true);
    routes.renderInbox.mockClear();

    await connectDevice("dev-a");
    await flush();

    expect(held()).toBe(false);
    expect(routes.renderInbox).toHaveBeenCalledTimes(1); // the route is rendered once, not once per device state

    expect(document.getElementById("devpick").hidden).toBe(false);
    expect(feed.items.map((item) => item.deviceId)).toContain("dev-a");
  });

  // Revoking a device is not the same as losing one: it is not coming back, so
  // nothing is kept for it. The account can run out of machines this way as
  // surely as by every bridge going, and the gate hears it the same way.
  it("holds the app when the last device is revoked", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();

    retireDevice("dev-a");
    expect(held()).toBe(false); // dev-b still answers
    retireDevice("dev-b");
    await flush();

    expect(held()).toBe(true);
    expect(liveIds()).toEqual([]);
  });

  // The RTCPeerConnection and the rendezvous are the app's, not the registry's:
  // a device let go of through the registry alone would leave both open for the
  // life of the tab, with both streams still pointed down the connection.
  it("closes the direct connection and the rendezvous a revoked device was riding", async () => {
    await connectEveryDevice();
    const session = lastSession("dev-a");
    const link = linksFor.get("dev-a");
    expect(contextFor("dev-a").peerLink).toBe(link);

    retireDevice("dev-a");
    await flush();

    expect(link.close).toHaveBeenCalledTimes(1);
    expect(session.peer).toHaveBeenLastCalledWith(null);
    expect(session.close).toHaveBeenCalled();
    expect(rendezvousFor.get("dev-a").close).toHaveBeenCalled();
    expect(contextFor("dev-a")).toBe(null);
  });

  // The key offered for this machine is not the key the account pinned, so the
  // machine answering is not the one that was paired. Asking again every few
  // seconds would offer the same pinned key to the same impostor and tell
  // nobody: this one error stops the client dead.
  it("stops for good on a device whose key is not the one this account pinned", async () => {
    impostors.add("dev-b");

    await connectEveryDevice();

    expect(liveIds()).toEqual(["dev-a"]); // the account's other machine is untouched
    // It is blocked like any other machine nothing could reach — and barred.
    expect(contextFor("dev-b").blocked).toBe("refused");
    const dialled = openedFor("dev-b").length;
    expect(dialled).toBe(1);

    await vi.advanceTimersByTimeAsync(60000);
    await openDeviceSessions().settled;
    await connectDevice("dev-b").catch(() => {});
    expect(openedFor("dev-b")).toHaveLength(dialled);
  });

  it("stops asking for a device whose key stopped matching while it was open", async () => {
    await connectEveryDevice();
    impostors.add("dev-a");

    await loseTheLink("dev-a");
    await connectDevice("dev-a").catch(() => {});
    const dialled = openedFor("dev-a").length;

    await vi.advanceTimersByTimeAsync(60000);
    await connectDevice("dev-a").catch(() => {});

    expect(openedFor("dev-a")).toHaveLength(dialled);
    expect(contextFor("dev-a").offline).toBe(true);
  });

  it("reopens only the device a retry names, and leaves the other session alone", async () => {
    await connectEveryDevice();
    const lost = lastSession("dev-a");
    const other = lastSession("dev-b");

    await loseTheLink("dev-a");
    await connectDevice("dev-a");
    await flush();

    expect(contextFor("dev-a").offline).toBe(false);
    expect(contextFor("dev-a").session).not.toBe(lost);
    // Nothing was asked of the device that never left.
    expect(openedFor("dev-b")).toHaveLength(1);
    expect(contextFor("dev-b").session).toBe(other);
    expect(other.close).not.toHaveBeenCalled();
    expect(homeContext()).toBe(contextFor("dev-a")); // and it is home again
  });

  // The account's list is all the gate has to go on, and a machine that came
  // online after boot is opened without a reload.
  it("connects a device that comes online after boot", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();
    expect(liveIds()).toEqual(["dev-a"]);
    captures.flush.mockClear(); // home already took the queue when it came up

    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    App.devices = devices;
    await openDeviceSessions().settled;
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
    expect(contextFor("dev-b").session).toBe(lastSession("dev-b"));
    // It joined the account, it did not take it over: home is untouched, and
    // the captures waiting for a device are not offered to it.
    expect(homeContext()?.session).toBe(lastSession("dev-a"));
    expect(captures.flush).not.toHaveBeenCalled();
  });

  // The api derives presence from a heartbeat, so a bridge that came back since
  // the last one is listed offline until the account catches up. An account
  // holding nothing has nothing else to try, so it asks anyway — once.
  it("dials a machine the account still calls offline when there is nothing else to try", async () => {
    devices = [away("dev-a", "Laptop")];
    App.devices = devices;

    const contexts = await connectEveryDevice();

    expect(contexts.map((context) => context.deviceId)).toEqual(["dev-a"]);
    expect(liveIds()).toEqual(["dev-a"]);
    expect(openedFor("dev-a")).toHaveLength(1);
  });

  it("guesses at a machine the account calls offline once, not on every re-read", async () => {
    devices = [away("dev-a", "Laptop")];
    App.devices = devices;
    unreachable.add("dev-a");

    await openDeviceSessions().settled;
    const guessed = openedFor("dev-a").length;
    await openDeviceSessions().settled; // the waiting screen re-reads the same stale list

    expect(guessed).toBe(1);
    expect(openedFor("dev-a")).toHaveLength(1);
    expect(contextFor("dev-a").blocked).toBe("unreached");
  });

  // A machine already being dialled must not be dialled again at itself: one
  // connect in flight is one connect, whoever asks for it.
  it("does not open a second session at the machine it is already dialling", async () => {
    devices = [away("dev-a", "Laptop")];
    App.devices = devices;
    slowMs.set("dev-a", 20);

    const sessions = openDeviceSessions();
    const pressedRetry = connectDevice("dev-a"); // a reader, mid-handshake
    await vi.advanceTimersByTimeAsync(20);
    await sessions.settled;
    await pressedRetry;
    await flush();

    expect(openedFor("dev-a")).toHaveLength(1);
    expect(contextFor("dev-a").session).toBe(lastSession("dev-a"));
  });

  // Another machine is answering, so the stale one is nobody's guess to make:
  // the presence poll opens it when the account says it is back.
  it("leaves a machine the account calls offline alone while another one answers", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();

    await openDeviceSessions().settled;

    expect(openedFor("dev-b")).toHaveLength(0);
    expect(liveIds()).toEqual(["dev-a"]);
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
    expect(homeContext()).toBe(null); // nothing is home until the device the pick names answers

    await vi.advanceTimersByTimeAsync(20);
    await sessions.settled;
    await flush();

    expect(homeContext()).toBe(contextFor("dev-b"));
    expect(homeContext().session).toBe(lastSession("dev-b"));
    expect(App.selectedDeviceId).toBe("dev-b"); // the pick itself is untouched
    expect(liveIds()).toEqual(["dev-a", "dev-b"]); // and nothing was closed
  });

  // The picker is not the only way home moves: the picked device landing after
  // another answered first moves it too. The terminals ride the home device's
  // channel, so whichever way home moved they have to be sent looking again.
  it("moves the terminals with home, however home moved", async () => {
    App.selectedDeviceId = "dev-b";
    slowMs.set("dev-b", 20);
    const sessions = openDeviceSessions();
    await sessions.first; // the quicker device landed, and it is not the picked one
    terminals.followTerminalDevice.mockClear();

    await vi.advanceTimersByTimeAsync(20);
    await sessions.settled;
    await flush();

    expect(homeContext()?.session).toBe(lastSession("dev-b"));
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
  });

  // Home is not a pointer anybody holds: it is what the account list and the
  // pick say. The picked machine's bridge going is news about home, so
  // everything that follows home — the terminals, the picker, the surfaces
  // about here — moves to the device that can still answer.
  it("home falls back to the first online device when the picked one goes away", async () => {
    App.selectedDeviceId = "dev-b";
    await connectEveryDevice();
    expect(homeContext()?.session).toBe(lastSession("dev-b"));
    terminals.followTerminalDevice.mockClear();

    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    deviceWentAway("dev-b");
    await flush();

    expect(homeContext()).toBe(contextFor("dev-a"));
    expect(homeContext().offline).toBe(false); // the device home moved to is answering
    // Away is not blocked: nothing here failed to reach it, its bridge has gone.
    expect(contextFor("dev-b").blocked).toBe(null);
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
  });

  it("moves home and the terminals to a device already open, and closes nothing", async () => {
    await connectEveryDevice();
    const stayed = lastSession("dev-a");
    const home = lastSession("dev-b");

    rememberSelectedDevice("dev-b");
    syncHome();

    expect(App.selectedDeviceId).toBe("dev-b");
    expect(homeContext()).toBe(contextFor("dev-b"));
    expect(homeContext().session).toBe(home);
    expect(terminals.followTerminalDevice).toHaveBeenCalled();
    expect(captures.flush).toHaveBeenCalled(); // the new home takes what nobody could send
    expect(stayed.close).not.toHaveBeenCalled();
    expect(contextFor("dev-a").session).toBe(stayed);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // The account has one control for home — Settings → Creation device — and it
  // is the only writer of the pick. The picker is a filter over the account's
  // list and says nothing about where creation goes, so a home move must not
  // shut it in the reader's hand.
  it("leaves an open device picker open when home moves", async () => {
    await connectEveryDevice();
    initDevicePicker();
    paintDevicePicker();
    document.querySelector(".device-picker-toggle").click();
    expect(document.querySelector(".device-picker-menu").hidden).toBe(false);

    chooseCreationDevice("dev-b");
    await flush();

    expect(document.querySelector(".device-picker-menu").hidden).toBe(false);
  });

  it("remembers the creation device the account picked, and follows it", async () => {
    await connectEveryDevice();
    captures.flush.mockClear();

    chooseCreationDevice("dev-b");

    expect(App.selectedDeviceId).toBe("dev-b");
    expect(localStorage.getItem("build.selectedDeviceId")).toBe("dev-b");
    expect(homeContext().deviceId).toBe("dev-b");
    expect(captures.flush).toHaveBeenCalledTimes(1);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // Home is what every surface about "here" is about: the composer's
  // destinations, the toolbar's projects, the capture decision page, the agent
  // rail. Each reads the home device's slice out of a snapshot as it arrives
  // and keeps it, so a home move that delivers nothing leaves them all on the
  // device the user just moved away from.
  it("delivers a snapshot when home moves, so the surfaces about here follow", async () => {
    await connectEveryDevice();
    startFeed(60000);
    await flush();
    let here = null;
    const stop = subscribeFeed((snapshot) => (here = deviceFeedView(snapshot)));
    expect(here.items.map((item) => item.deviceId)).toEqual(["dev-a"]);

    rememberSelectedDevice("dev-b");
    syncHome();

    expect(here.items.map((item) => item.deviceId)).toEqual(["dev-b"]);
    stop();
  });

  it("offers the new home device's projects the moment home moves", async () => {
    await connectEveryDevice();
    initCompose();
    startFeed(60000);
    await flush();
    expect(projectsOffered()).toEqual(["dev-a repo"]);

    rememberSelectedDevice("dev-b");
    syncHome();

    expect(projectsOffered()).toEqual(["dev-b repo"]);
  });

  // The picked device coming back opens a session, and the landing already
  // names it home. Taking it in hand a second time on the way out re-points
  // what is already pointed, repaints the picker, delivers a second identical
  // snapshot and offers the capture queue twice.
  it("takes a newly opened home device in hand once, not twice", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();
    captures.flush.mockClear();
    rememberSelectedDevice("dev-b"); // creation is to go to the device that is away

    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    App.devices = devices;
    await openDeviceSessions().settled;
    await flush();

    expect(homeContext()?.session).toBe(lastSession("dev-b"));
    expect(captures.flush).toHaveBeenCalledTimes(1);
  });

  // Presence is the api's while the app is open (rule 6), and the app being
  // open is what the gate says: it starts the poll when it hands the page back
  // and stops it when it takes the page again.
  it("follows the account's presence while the app is open, and stops when the gate takes it", async () => {
    await connectEveryDevice();
    account.fetchDevices.mockClear();

    await vi.advanceTimersByTimeAsync(15000);
    expect(account.fetchDevices).toHaveBeenCalledTimes(1);

    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(held()).toBe(true);
    account.fetchDevices.mockClear();
    // The waiting screen's own 3 s poll is the gate's, and this case is not
    // about it: what must not happen is the app's 15 s one going on underneath.
    document.getElementById("watchmsg"); // the waiting screen is up
    await vi.advanceTimersByTimeAsync(15000);

    expect(account.fetchDevices).not.toHaveBeenCalled();
  });

  // The whole of the late join, end to end: the account says a machine that was
  // down is up, and the poll opens it.
  it("takes up a machine the account has started calling online again", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();
    expect(liveIds()).toEqual(["dev-a"]);

    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    await vi.advanceTimersByTimeAsync(15000);
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // And the other way: the bridge stopped posting its heartbeat, the api
  // stopped calling it online, and the machine this client holds goes away —
  // plainly away, not blocked: nothing here failed to reach it.
  it("marks a machine away when the account stops calling it online", async () => {
    await connectEveryDevice();

    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    await vi.advanceTimersByTimeAsync(15000);
    await flush();

    expect(liveIds()).toEqual(["dev-a"]);
    expect(contextFor("dev-b").offline).toBe(true);
    expect(contextFor("dev-b").blocked).toBe(null);
    expect(held()).toBe(false);
  });

  it("pauses only the calls of the device that went offline", async () => {
    await connectEveryDevice();
    const lost = openedFor("dev-a").at(-1);
    const kept = openedFor("dev-b").at(-1);

    await loseTheLink("dev-a");

    expect(lost.isPaused()).toBe(true);
    expect(kept.isPaused()).toBe(false);
    expect(contextFor("dev-b").offline).toBe(false);
  });

  // What was waiting on a wire that is not coming is refused in the words the
  // surfaces over that machine show, rather than held for a channel that will
  // never carry (rule 3).
  it("refuses what the blocked device was holding, in its own words", async () => {
    await connectEveryDevice();
    const session = lastSession("dev-a");

    await loseTheLink("dev-a");

    expect(session.fail).toHaveBeenCalledTimes(1);
    expect(session.fail.mock.calls[0][0].message).toBe("Device not reachable");
    expect(session.close).toHaveBeenCalled();
  });

  it("marks a machine plainly away when its bridge is simply gone", async () => {
    await connectEveryDevice();
    const session = lastSession("dev-a");

    deviceWentAway("dev-a");
    await flush();

    expect(contextFor("dev-a").offline).toBe(true);
    expect(contextFor("dev-a").blocked).toBe(null);
    expect(session.fail.mock.calls[0][0].message).toBe("Device offline");
    expect(goOffline("dev-a")).toBe(undefined); // and losing it again is no news
  });
});
