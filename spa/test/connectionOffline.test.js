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
let boardRows = new Map();

vi.mock("../src/core/rendezvous.js", () => ({
  createRelayRendezvous: (options) => makeRendezvous(options),
}));
vi.mock("../src/core/session.js", () => ({
  openSession: (options) => wire.openSession(options),
}));
vi.mock("../src/core/peerLink.js", () => ({ openPeerLink: (options) => wire.openPeerLink(options) }));
// The rail paints from the cache and the sync layer fills it. IndexedDB
// settles on the event loop this suite has faked, so the store is the
// in-memory double instead — same addresses, same announcements.
vi.mock("../src/core/localCache.js", () => import("./memoryCache.js"));
const account = vi.hoisted(() => ({ fetchDevices: null }));
vi.mock("../src/api.js", () => ({
  fetchGatewayToken: async () => "tok",
  fetchIceServers: async () => [],
  fetchDevices: (...args) => account.fetchDevices(...args),
}));
vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice: (...args) => terminals.followTerminalDevice(...args),
  resetTerminalManager: () => {},
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
const {
  canAnswer, contextFor, deviceFeedView, homeContext, knownContexts, knownDeviceContext, liveContexts, resetDeviceContexts,
} = await import("../src/core/deviceContexts.js");
const {
  chooseCreationDevice,
  connectDevice,
  deviceRecoverySnapshot,
  deviceWentAway,
  goOffline,
  openDeviceSessions,
  retireDevice,
  syncDeviceRecoveryPresence,
  syncHome,
} = await import("../src/connection.js");
const { initDevicePicker, paintDevicePicker, stopWatchingPresence } = await import("../src/devices.js");
const { clearMemoryCacheRecords, DEVICES_ADDRESS, writeCached } = await import("./memoryCache.js");
const { startFeed, stopFeed, subscribeFeed } = await import("../src/core/taskFeed.js");
const { startCacheSync, stopCacheSync, syncDevice } = await import("../src/core/cacheSync.js");
const { dispatchChangeEvent } = await import("../src/core/changeEvents.js");
const { allDevicesOfflineText, deviceUnreachableText, devicesBlockedText } = await import("../src/core/text.js");
const { mountInboxList } = await import("../src/core/inboxView.js");
const { initCompose, openCompose } = await import("../src/core/composeView.js");
const { holdAppWhileNoDeviceAnswers } = await import("../src/views/gate.js");
const { mountConnectionStatus, unmountConnectionStatus } = await import("../src/connectionStatus.js");

const flush = () => vi.advanceTimersByTimeAsync(0);
const reachNextRecoveryAttempt = async () => {
  const waiting = deviceRecoverySnapshot().filter((record) => record.nextAttemptAt != null);
  const delay = Math.max(...waiting.map((record) => record.nextAttemptAt - Date.now()), 0);
  await vi.advanceTimersByTimeAsync(delay + 1);
  for (let index = 0; index < 6; index += 1) await flush();
};

const online = (id, name) => ({
  id,
  name,
  status: "online",
  fingerprint: `${id}-fingerprint`,
  last_seen_at: new Date(Date.now()).toISOString(),
});
/** The same paired device, as the account lists it while its bridge is down —
 *  beating moments ago, so the offline it wears may be the list not having
 *  caught up (core/devicePolicy.js). */
const away = (id, name) => ({ ...online(id, name), status: "offline" });
/** A machine the account has not heard from in days. Nothing about that listing
 *  is the api lagging. */
const longGone = (id, name) => ({
  ...away(id, name),
  last_seen_at: new Date(Date.now() - 4 * 24 * 60 * 60 * 1000).toISOString(),
});

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

/** The recovery status every real peer link carries (core/peerLink.js): whether
 *  it is renegotiating right now. The app session's path probe stands down while
 *  it is, so a fake link has to be able to say. */
const fakeRecovery = (recovering = false) => ({
  snapshot: () => ({ epoch: 0, recovering }),
  subscribe: () => () => {},
});

/** A direct connection as the connection layer uses it: two channels that can
 *  say they closed, its recovery status, and a way to close the pair. */
function fakePeerLink(deviceId) {
  const carrier = () => {
    const listeners = new Set();
    return {
      onClose: (fn) => listeners.add(fn),
      // A wire says why it went where it was shut on purpose: the terminals'
      // own judgement that nothing was carrying is the one reason worth the
      // whole connection.
      drop: (reason = null) => listeners.forEach((fn) => fn(reason)),
    };
  };
  const link = {
    app: carrier(),
    term: carrier(),
    recovery: fakeRecovery(),
    // Every real link tells its owner when the way it is carrying changes, so
    // the ring can redraw the word (core/peerLink.js, issue #31).
    onPathChanged: vi.fn(() => () => {}),
    // A failed path put right by its restart, which may have landed on a
    // bridge that restarted under it (#123).
    restored: new Set(),
    onRestored: vi.fn((fn) => {
      link.restored.add(fn);
      return () => link.restored.delete(fn);
    }),
    restore: () => link.restored.forEach((fn) => fn()),
    transportPath: () => null,
    close: vi.fn(),
  };
  linksFor.set(deviceId, link);
  return link;
}

/** A bridge session as the connection layer uses it: something to call, a
 *  channel to ride, and a way to say it is gone. Taken off its channel — by the
 *  channel closing, or by the layer handing it back — it reports the loss
 *  exactly as the real session does: nothing else ever carried it. */
function fakeSession(deviceId, onLost = () => {}) {
  let carrierChanged = () => {};
  const session = {
    deviceId,
    sessionId: `session-${deviceId}`,
    call: vi.fn(async (method) => {
      if (method === "session.hello" && greetings.has(deviceId)) return greetings.get(deviceId)();
      if (method === "board.list") return { items: [boardRows.get(deviceId) || {
        id: `${deviceId}-row`, project_id: "proj-1", title: "Work",
      }] };
      if (method === "project.list") return { projects: [{ project_id: "proj-1", name: `${deviceId} repo` }] };
      // The rail lists workspaces, so a machine with none paints no rows at
      // all: each one keeps a checkout of the project both machines name.
      if (method === "workspace.list")
        return { workspaces: [{ workspace_id: `${deviceId}-workspace`, project_id: "proj-1", title: "Work", state: "running" }] };
      return { deviceId };
    }),
    peer: vi.fn((carrier) => {
      if (!carrier && !session.closed) onLost();
      return carrier ? carrierChanged() : undefined;
    }),
    fail: vi.fn(),
    // The deliberate path probe another channel's verdict is checked against
    // (#60). A live path by default: the terminals being wrong about the app
    // channel is the ordinary case.
    probePath: vi.fn(async () => "alive"),
    installAdapter: vi.fn((selection) => selection?.create?.(session.call) || null),
    adapter: vi.fn(() => null),
    onPush: () => () => {},
    onCarrier: vi.fn((fn) => { carrierChanged = fn; }),
    // Where the session learns whether its peer is renegotiating, so its path
    // probe can stand down for a restart (core/session.js).
    watchRecovery: vi.fn(),
    fireCarrier: () => carrierChanged(),
    reattachSignaling: vi.fn(async () => {}),
    // Whether a restarted path carries this session (#123). The answer names
    // the machine so a case can tell which session a link was handed.
    confirmCarried: vi.fn(async () => `carried by ${deviceId}`),
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

beforeEach(async () => {
  vi.useFakeTimers();
  clearMemoryCacheRecords();
  localStorage.clear();
  resetApplication();
  stopFeed();
  document.body.innerHTML =
    '<div id="root"></div><div id="devpick"></div><div id="compose"></div><div id="inbox-list"></div>' +
    '<div id="connection-status" hidden></div><div id="connection-announcement" aria-live="polite"></div>';
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
  boardRows = new Map();
  handedOut.clear();
  rendezvousFor.clear();
  linksFor.clear();
  feed = null;
  devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
  App.devices = devices;
  await writeCached(DEVICES_ADDRESS, devices);
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
  wire.openPeerLink.mockImplementation(async ({ signal }) => {
    const { deviceId } = await signal("rtc.offer", { sdp: "v=0" });
    if (unlinkable.has(deviceId)) throw Object.assign(new Error("no channels"), { blockedReason: "timeout" });
    return fakePeerLink(deviceId);
  });
  unsubscribe = subscribeFeed((snapshot) => (feed = snapshot));
});

afterEach(() => {
  stopCacheSync();
  unmountConnectionStatus();
  delete globalThis.RTCPeerConnection;
  stopWatchingPresence();
  unsubscribe();
  stopFeed();
  resetApplication();
  vi.clearAllTimers();
  vi.useRealTimers();
});

/** Stand the cache-first client up where a test did not come in through the
 *  gate: the sync layer reads every device that can answer, and the feed
 *  paints what lands on disk. The feed itself reads nothing. */
async function paintFeed() {
  startCacheSync();
  startFeed();
  for (let index = 0; index < 12; index += 1) await flush();
}

/** Open the composer's manual panel, read the projects it offers, and close it
 *  again — the box takes its destinations from the home device's slice of the
 *  feed as that slice is delivered, which is what a home move has to move. */
async function projectsOffered(expected) {
  openCompose();
  document.querySelector("#compose-advanced").click();
  await vi.waitFor(() => expect([...document.querySelectorAll("#compose-project option")].map((option) => option.textContent)).toEqual(expected));
  const names = [...document.querySelectorAll("#compose-project option")].map((option) => option.textContent);
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  return names;
}

/** Whether the gate is holding the app: nothing can answer, so there is no
 *  route to stand on and the waiting screen owns the page. An app that has
 *  been painted is never held — it stands on its cache and wears the mark
 *  below instead (plan stage 6). */
const held = () => document.body.classList.contains("gated");

/** What the picker says while nothing the account has can answer. */
const nothingAnswersOnPicker = () => Boolean(document.querySelector(".device-picker-unreachable"));

/** The account with nothing that can answer and no shell painted yet: a cold
 *  boot whose cache had nothing in it, which is the one state the whole-page
 *  waiting screen is still for. */
async function holdWithNothingPainted() {
  App.gated = true;
  holdAppWhileNoDeviceAnswers();
  await flush();
}

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
    await paintFeed();

    expect(boardReads("dev-a")).toBe(0);
    expect(boardReads("dev-b")).toBeGreaterThan(0); // the other machine is not held up
    expect(rendezvousFor.get("dev-a").close).not.toHaveBeenCalled();
    expect(rendezvousFor.get("dev-b").close).toHaveBeenCalled();

    greet({});
    await flush();

    expect(boardReads("dev-a")).toBeGreaterThan(0);
    expect(rendezvousFor.get("dev-a").close).toHaveBeenCalled();
  });

  // #123: an ICE restart can land on a bridge process that restarted under the
  // session. It carries the same id and key and holds nothing else — no
  // greeting, so no subscriptions and no pushes — and the channel it rides has
  // not changed, so no carrier change greets it. The restored path does.
  // (The greeting is what replays the subscriptions and refetches every
  // surface: core/changeEvents.js greetBridge, tested there.)
  it("greets the machine again when its failed path is restored", async () => {
    greetings.set("dev-a", async () => ({
      api_version: "1.22.0",
      push_events: true,
      capabilities: ["changes.subscriptions", "issues.attachments"],
      changes: { kinds: ["state", "thread"] },
    }));
    const row = (agentId) => ({
      kind: "branch", run_id: "dev-a-row", project_id: "proj-1", branch: "build/demo",
      worktree_id: "dev-a-worktree", state: "building", agents: [{ id: agentId }],
    });
    boardRows.set("dev-a", row("agent-before"));
    await connectEveryDevice();
    await paintFeed();
    const session = lastSession("dev-a");
    const hellos = () => session.call.mock.calls.filter(([method]) => method === "session.hello").length;
    const greeted = hellos();
    const boardBefore = boardReads("dev-a");
    expect(greeted).toBeGreaterThan(0);
    expect(boardBefore).toBeGreaterThan(0);
    expect(contextFor("dev-a").adapter.capabilities.changes.subscriptions).toBe(true);
    expect(feed.items.find((item) => item.deviceId === "dev-a")?.agents).toEqual([{ id: "agent-before" }]);

    boardRows.set("dev-a", row("agent-after"));
    linksFor.get("dev-a").restore();
    await flush();

    await vi.waitFor(() => expect(hellos()).toBeGreaterThan(greeted));
    expect(lastSession("dev-a")).toBe(session); // the same session, greeted again
    expect(contextFor("dev-a").offline).toBe(false);
    expect(contextFor("dev-a").adapter.capabilities.changes.subscriptions).toBe(true);
    expect(boardReads("dev-a")).toBeGreaterThan(boardBefore); // the bridge may have missed changes while the path was down
    await vi.waitFor(() => expect(feed.items.find((item) => item.deviceId === "dev-a")?.agents)
      .toEqual([{ id: "agent-after" }]));

    const boardAfter = boardReads("dev-a");
    expect(dispatchChangeEvent({
      type: "changes", subscription_id: "s-inbox",
      items: [{ entity_id: "dev-a-row", state: row("agent-pushed") }],
    }, "dev-a")).toBe(true);
    await vi.waitFor(() => expect(feed.items.find((item) => item.deviceId === "dev-a")?.agents)
      .toEqual([{ id: "agent-pushed" }]));
    expect(boardReads("dev-a")).toBe(boardAfter); // the pushed body writes the cache without another list read
  });

  it("ignores a restored-path callback from a replaced session", async () => {
    await connectEveryDevice();
    await paintFeed();
    const oldSession = lastSession("dev-a");
    const oldLink = linksFor.get("dev-a");

    goOffline("dev-a");
    await connectDevice("dev-a");
    await flush();
    const currentSession = lastSession("dev-a");
    expect(currentSession).not.toBe(oldSession);
    const boardBefore = boardReads("dev-a");
    const oldHelloBefore = oldSession.call.mock.calls.filter(([method]) => method === "session.hello").length;

    oldLink.restore();
    await flush();

    expect(lastSession("dev-a")).toBe(currentSession);
    expect(boardReads("dev-a")).toBe(boardBefore);
    expect(oldSession.call.mock.calls.filter(([method]) => method === "session.hello")).toHaveLength(oldHelloBefore);
  });

  // The link only knows a restart landed on a dead process if it can ask the
  // session. Without the question it takes every restart as carried.
  it("hands each peer link its own session's carry check", async () => {
    await connectEveryDevice();
    const handed = wire.openPeerLink.mock.calls.map(([options]) => options.confirmCarried);
    expect(handed.length).toBeGreaterThan(0);
    expect(handed.every((ask) => typeof ask === "function")).toBe(true);
    await expect(handed.at(-1)()).resolves.toMatch(/^carried by dev-/);
    const asked = [...handedOut.values()].flat().filter((one) => one.confirmCarried.mock.calls.length > 0);
    expect(asked).toHaveLength(1);
    expect(await handed.at(-1)()).toBe(`carried by ${asked[0].deviceId}`);
  });

  it("closes a failed handoff instead of treating a network error as an old bridge", async () => {
    greetings.set("dev-a", async () => { throw new Error("request timed out"); });
    await connectEveryDevice();
    expect(boardReads("dev-a")).toBe(0);
    expect(contextFor("dev-a").offline).toBe(true);
    expect(lastSession("dev-a").closed).toBe(true);
    expect(linksFor.get("dev-a").close).toHaveBeenCalled();
    expect(boardReads("dev-b")).toBeGreaterThan(0);
  });

  // A bridge that predates `session.hello` refuses the verb, and that refusal is
  // the whole of the feature detection: it has answered. A pass waits for that
  // machine's greeting before it asks it anything, and waiting on a greeting
  // must not be waiting on one that can never arrive — so the pass answers,
  // and the machine that did greet is read whatever this one did.
  it("does not leave a pass waiting on a greeting that can never arrive", async () => {
    greetings.set("dev-a", async () => {
      throw new Error("unknown method: session.hello");
    });

    await connectEveryDevice();
    await paintFeed();

    await expect(syncDevice("dev-a")).resolves.toBe(false);
    expect(boardReads("dev-b")).toBeGreaterThan(0);
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
    await paintFeed();

    unlinkable.add("dev-a");
    await loseTheLink("dev-a");

    expect(contextFor("dev-a").offline).toBe(true);
    expect(contextFor("dev-a").blocked).toBe("timeout");
    expect(knownContexts().map((context) => context.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(liveIds()).toEqual(["dev-b"]);
    // Its rows are still the account's rows — greyed by the rail, not removed.
    expect(feed.items.map((item) => item.deviceId)).toEqual(["dev-a", "dev-b"]);
    expect(held()).toBe(false); // the account still has a machine to stand on
    // The account lists dev-a online, but it has answered nothing: creation goes
    // to the machine that can take it rather than refusing on the one that is
    // merely listed first.
    expect(homeContext()).toBe(contextFor("dev-b"));
    expect(canAnswer(homeContext())).toBe(true);
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
    await paintFeed();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));
    expect(rowOn("dev-a")).toBeTruthy();

    unlinkable.add("dev-a");
    await loseTheLink("dev-a");

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);
    expect(rowOn("dev-b").classList.contains("inbox-offline")).toBe(false);
  });

  // And it comes back the same way: a device that answers again ungreys its
  // rows without waiting for anything to move.
  it("ungreys a device's rows the moment a retry lands it", async () => {
    await connectEveryDevice();
    mountInboxList();
    await paintFeed();
    const rowOn = (deviceId) =>
      [...document.querySelectorAll("#inbox-list .inbox-entry")].find((row) => row.dataset.key.includes(deviceId));

    unlinkable.add("dev-a");
    await loseTheLink("dev-a");
    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(true);

    unlinkable.delete("dev-a");
    await connectDevice("dev-a");
    await flush();

    expect(rowOn("dev-a").classList.contains("inbox-offline")).toBe(false);
    expect(contextFor("dev-a").blocked).toBe(null);
  });

  it("keeps redialling a blocked machine while the account still calls it online", async () => {
    unlinkable.add("dev-b");
    await connectEveryDevice();
    const dialled = openedFor("dev-b").length;

    await openDeviceSessions().settled;
    await vi.advanceTimersByTimeAsync(60000);

    expect(openedFor("dev-b").length).toBeGreaterThan(dialled);

    unlinkable.delete("dev-b");
    await reachNextRecoveryAttempt();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // The reader keeps their page when the last machine goes. Everything on it
  // came off the cache and is still true of the last thing Build saw, so the
  // account running out of machines is said on the picker rather than by
  // taking the app away (plan stage 6).
  it("keeps the painted app when the last device goes, and says so on the picker", async () => {
    initDevicePicker();
    await connectEveryDevice();
    // The bridges went, so the account stopped calling them online — which is
    // what "offline" on this screen means.
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);
    syncDeviceRecoveryPresence(devices);

    await loseTheLink("dev-a");
    await loseTheLink("dev-b");

    expect(held()).toBe(false);
    expect(document.getElementById("waitlist")).toBeNull();
    expect(nothingAnswersOnPicker()).toBe(true);
  });

  it("keeps the shell mounted and schedules recovery when every online machine is unreachable", async () => {
    mountConnectionStatus();
    unlinkable.add("dev-a");
    unlinkable.add("dev-b");

    await connectEveryDevice();

    expect(held()).toBe(false);
    expect(deviceRecoverySnapshot().map(({ deviceId, status, failedAttempts }) => ({ deviceId, status, failedAttempts }))).toEqual([
      { deviceId: "dev-a", status: "waiting", failedAttempts: 1 },
      { deviceId: "dev-b", status: "waiting", failedAttempts: 1 },
    ]);
    // The ring is the button inside the icon's box (spa/src/connectionStatus.js).
    const ring = document.querySelector("#connection-status .connection-status");
    expect(ring.dataset.state).toBe("waiting");
    expect(ring.getAttribute("aria-label")).toContain("Reconnecting to Laptop and Desktop");

    unlinkable.delete("dev-a");
    await reachNextRecoveryAttempt();

    expect(held()).toBe(false);
    expect(openedFor("dev-a").length).toBeGreaterThan(1);
    expect(liveIds()).toEqual(["dev-a"]);
  });

  // The screen's account-wide Retry is for exactly the state it is standing in:
  // every machine the account calls online is blocked, and a block is what
  // stops this layer asking for one again (rule 3). Pressing it has to clear
  // them and dial — routing through a boot that refuses every blocked machine
  // it finds asks nothing of anybody.
  it("dials every blocked machine again when the reader retries now", async () => {
    unlinkable.add("dev-a");
    unlinkable.add("dev-b");
    await connectEveryDevice();
    expect(held()).toBe(false);
    const asked = openedFor("dev-a").length;

    unlinkable.clear();
    await openDeviceSessions({ retry: true }).settled;

    expect(openedFor("dev-a").length).toBeGreaterThan(asked);
    expect(held()).toBe(false);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // The account's list is up to a heartbeat window out of date (rule 6), so the
  // machines under this screen are routinely ones it still calls offline. The
  // Retry clears every block it finds, and a clear that dials nobody is the
  // worst of both: the row loses the reason it was wearing and no machine is
  // asked. What it cleared is what it dials.
  it("dials the machines it unblocked even while the account lists them offline", async () => {
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    unreachable.add("dev-a");
    unreachable.add("dev-b");
    await connectEveryDevice();
    expect(contextFor("dev-a").blocked).toBe("unreached");
    expect(contextFor("dev-b").blocked).toBe("unreached");
    const asked = { a: openedFor("dev-a").length, b: openedFor("dev-b").length };

    unreachable.clear();
    await openDeviceSessions({ retry: true }).settled;

    expect(openedFor("dev-a").length).toBe(asked.a + 1);
    expect(openedFor("dev-b").length).toBe(asked.b + 1);
    expect(held()).toBe(false);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // The account's list lags by a heartbeat window, so a boot that has nothing
  // answering asks the machines it calls offline anyway. That hedge is against
  // a window of lag and nothing more: on 2026-09-19 it dialled a Mac mini the
  // api had not heard from in four days, over and over, and the relay answered
  // `rejected session to device` every time.
  it("does not dial a machine the account has not heard from in days", async () => {
    devices = [longGone("dev-a", "Laptop"), longGone("dev-b", "Desktop")];
    App.devices = devices;

    await connectEveryDevice();

    expect(openedFor("dev-a")).toEqual([]);
    expect(openedFor("dev-b")).toEqual([]);
    expect(knownContexts()).toEqual([]);
  });

  // The hedge itself still stands: a machine that beat moments ago is one whose
  // offline may simply not have caught up, and nothing else is answering.
  it("still dials a machine listed offline whose last beat is inside the window", async () => {
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;

    await connectEveryDevice();

    expect(openedFor("dev-a").length).toBe(1);
    expect(openedFor("dev-b").length).toBe(1);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // And when that dial fails too, the machine goes back to wearing a reason:
  // the clear is only ever as long as the attempt it was made for.
  it("re-blocks an unblocked machine the account-wide Retry still could not reach", async () => {
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    unreachable.add("dev-a");
    unreachable.add("dev-b");
    await connectEveryDevice();

    unreachable.delete("dev-b");
    unlinkable.add("dev-a");
    unreachable.delete("dev-a");
    await openDeviceSessions({ retry: true }).settled;
    await flush();

    expect(contextFor("dev-a").blocked).toBe("timeout");
    expect(liveIds()).toEqual(["dev-b"]);
  });

  // And one that cannot be reached this time either is blocked again, with the
  // reason it failed with now.
  it("blocks the machines the account-wide Retry still could not reach", async () => {
    unlinkable.add("dev-a");
    unlinkable.add("dev-b");
    await connectEveryDevice();
    unlinkable.delete("dev-b");
    unreachable.add("dev-b");

    await openDeviceSessions({ retry: true }).settled;
    await flush();

    expect(held()).toBe(false);
    expect(contextFor("dev-a").blocked).toBe("timeout");
    expect(contextFor("dev-b").blocked).toBe("unreached");
  });

  it("keeps recovery visible when a retry could not reach the machine either", async () => {
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;
    unlinkable.add("dev-a");
    await connectEveryDevice();
    expect(held()).toBe(false);

    unlinkable.delete("dev-a");
    unreachable.add("dev-a");
    await openDeviceSessions({ retry: true }).settled;

    expect(held()).toBe(false);
    expect(contextFor("dev-a").blocked).toBe("unreached");
    expect(deviceRecoverySnapshot("dev-a").failedAttempts).toBe(1);
  });

  // Which sentence this is, is the account's question rather than this client's.
  // A machine that was already down at boot was never opened here and has no
  // context, so counting contexts called an account of two machines an account
  // of one — and named whichever one this client happened to hold.
  it("says every device is offline when one of them was already down at boot", async () => {
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectEveryDevice();
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);

    await loseTheLink("dev-a");
    await holdWithNothingPainted();

    expect(held()).toBe(true);
    expect(waitingNote()).toContain(allDevicesOfflineText());
  });

  it("keeps naming the one device on the account, and when it went unreachable", async () => {
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;
    await connectEveryDevice();
    devices = [away("dev-a", "Laptop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);

    await loseTheLink("dev-a");
    await holdWithNothingPainted();

    expect(held()).toBe(true);
    expect(waitingHeading()).toBe("Waiting for your device");
    expect(waitingNote()).toContain(deviceUnreachableText("Laptop", contextFor("dev-a").offlineSince));
  });

  it("preserves the mounted view while online devices reconnect", async () => {
    await connectEveryDevice();
    await paintFeed();
    const poll = { dispose: vi.fn() };
    const viewDispose = vi.fn();
    App.poll = poll;
    App.viewDispose = viewDispose;

    await loseTheLink("dev-a");
    await loseTheLink("dev-b");

    expect(held()).toBe(false);
    expect(poll.dispose).not.toHaveBeenCalled();
    expect(viewDispose).not.toHaveBeenCalled();
    expect(App.poll).toBe(poll);
    expect(App.viewDispose).toBe(viewDispose);
  });

  // The screen's foot is one line about what the app is doing and the two things
  // a reader can do about it.
  it("lays the waiting screen's foot out as one row", async () => {
    await connectEveryDevice();
    await paintFeed();
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    await holdWithNothingPainted();
    expect(held()).toBe(true);

    const foot = document.getElementById("watchmsg").parentElement;
    expect(foot.className.split(/\s+/)).toContain("wait-row");
    expect([...foot.children].map((child) => child.id)).toEqual(["watchmsg", "retrybtn", "addmore"]);
    // Where each of them sits is the sheet's to say, not the markup's.
    expect(document.getElementById("retrybtn").getAttribute("style")).toBe(null);
  });

  // An account with nothing that can answer is watched at the gate's own
  // cadence, whatever is on screen. A machine that comes back some other way —
  // a retry landing through the hold listener — hands the app straight back,
  // and the watch left armed would re-enter the app over a reader already
  // standing in it.
  it("stops the gate's watch when a device lands another way", async () => {
    await connectEveryDevice();
    await paintFeed();
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);
    // Neither bridge is back yet, so nothing can answer and the watch is armed
    // over the page the reader keeps — which is the state this is about.
    unreachable.add("dev-a");
    unreachable.add("dev-b");
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(App._watch).not.toBe(null);
    routes.renderInbox.mockClear();

    // The bridge comes back, and a retry on that machine is what lands it.
    unreachable.clear();
    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    await connectDevice("dev-a");
    await flush();
    expect(App._watch).toBe(null);
    expect(routes.renderInbox).toHaveBeenCalledTimes(1);
    account.fetchDevices.mockClear();

    await vi.advanceTimersByTimeAsync(7000);

    expect(account.fetchDevices).not.toHaveBeenCalled();
    expect(routes.renderInbox).toHaveBeenCalledTimes(1);
  });

  // And the way back is the same signal: the machine that answers hands the
  // reader their route back, with the feed reading it again — no reload.
  it("hands the app back the moment one device answers again", async () => {
    initDevicePicker();
    await connectEveryDevice();
    await paintFeed();
    devices = [away("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    syncDeviceRecoveryPresence(devices);
    await loseTheLink("dev-a");
    await loseTheLink("dev-b");
    expect(nothingAnswersOnPicker()).toBe(true);
    routes.renderInbox.mockClear();

    await connectDevice("dev-a");
    await flush();

    expect(held()).toBe(false);
    expect(nothingAnswersOnPicker()).toBe(false);
    expect(routes.renderInbox).toHaveBeenCalledTimes(1); // the route is rendered once, not once per device state

    expect(document.getElementById("devpick").hidden).toBe(false);
    expect(feed.items.map((item) => item.deviceId)).toContain("dev-a");
  });

  // A shell entered while its machines are still being dialled paints the
  // route off the records, and that page stands on a session-less context
  // (core/surfaceContext.js). The context is not the machine landing: the
  // shell's one hand-back — feed, sync, route — is still owed to the first
  // machine that answers.
  it("still hands the app back when a page stood on a machine's records before it answered", async () => {
    initDevicePicker();
    knownDeviceContext("dev-a");
    holdAppWhileNoDeviceAnswers();
    await flush();
    routes.renderInbox.mockClear();

    await connectDevice("dev-a");
    await flush();

    expect(routes.renderInbox).toHaveBeenCalledTimes(1);
    expect(feed.items.map((item) => item.deviceId)).toContain("dev-a");
  });

  it("does not misreport revoked online rows as an all-offline account", async () => {
    await connectEveryDevice();
    await paintFeed();

    retireDevice("dev-a");
    expect(held()).toBe(false); // dev-b still answers
    retireDevice("dev-b");
    await flush();

    expect(held()).toBe(false);
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

  // A device let go of while this layer was dialling it: the account no longer
  // has that machine, so nothing the dial goes on to say about it is news. A
  // block would put the retired machine back in the rail — greyed, with a Retry
  // that dials a device the account does not have — for the life of the tab.
  it("does not resurrect a machine the account let go of while it was being dialled", async () => {
    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    App.devices = devices;
    slowMs.set("dev-b", 20);
    unlinkable.add("dev-b");
    const dial = connectDevice("dev-b").catch((error) => error);
    await flush();

    retireDevice("dev-b");
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;

    await vi.advanceTimersByTimeAsync(20);
    await dial;
    await flush();

    expect(contextFor("dev-b")).toBe(null);
    expect(knownContexts().map((context) => context.deviceId)).toEqual([]);
  });

  // And a dial that LANDS on a retired machine lands nothing: the session it
  // opened is closed rather than adopted into a context for a device nobody has.
  it("closes a session that answers for a machine the account let go of mid-dial", async () => {
    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    App.devices = devices;
    slowMs.set("dev-b", 20);
    const dial = connectDevice("dev-b").catch((error) => error);
    await flush();

    retireDevice("dev-b");
    devices = [online("dev-a", "Laptop")];
    App.devices = devices;

    await vi.advanceTimersByTimeAsync(20);
    await dial;
    await flush();

    expect(contextFor("dev-b")).toBe(null);
    expect(lastSession("dev-b").close).toHaveBeenCalled();
  });

  // Retiring a machine calls its dial off, and the account can hand the same
  // machine back — a revoke undone, an api list that still names it — while
  // that dial is still in flight. What the late one lands is one dial's
  // business: the session and the connection IT opened, closed, and not a
  // thing belonging to the machine as the account now has it.
  it("closes only what it opened when a newer dial has already landed the machine", async () => {
    devices = [online("dev-a", "Laptop"), online("dev-b", "Desktop")];
    App.devices = devices;
    slowMs.set("dev-b", 20);
    const late = connectDevice("dev-b").catch((error) => error);
    await flush();

    retireDevice("dev-b"); // the dial at dev-b is called off, and it is let go

    slowMs.set("dev-b", 5); // the account has it again, and this dial lands first
    const fresh = connectDevice("dev-b");
    await vi.advanceTimersByTimeAsync(5);
    await fresh;
    const landed = lastSession("dev-b");
    const landedLink = linksFor.get("dev-b");
    expect(contextFor("dev-b").session).toBe(landed);

    await vi.advanceTimersByTimeAsync(20);
    await late;
    await flush();

    const stale = lastSession("dev-b");
    expect(stale).not.toBe(landed);
    expect(stale.close).toHaveBeenCalled();
    // Cancellation won before the stale session could begin its next async
    // phase, so it did not negotiate a peer link of its own.
    expect(linksFor.get("dev-b")).toBe(landedLink);
    // …and the machine the account has is left exactly as it was landed.
    expect(contextFor("dev-b")?.session).toBe(landed);
    expect(landed.close).not.toHaveBeenCalled();
    expect(landedLink.close).not.toHaveBeenCalled();
  });

  it("cancels a delayed greeting on reset without touching a fresh same-id attempt", async () => {
    let finishOldGreeting;
    greetings.set("dev-a", () => new Promise((resolve) => { finishOldGreeting = resolve; }));
    const oldConnection = connectDevice("dev-a");
    await flush();
    const oldSession = lastSession("dev-a");
    const oldLink = linksFor.get("dev-a");

    resetApplication();
    await expect(oldConnection).rejects.toThrow(/retired|cancelled/);
    greetings.delete("dev-a");
    const freshContext = await connectDevice("dev-a");
    const freshSession = lastSession("dev-a");
    const freshLink = linksFor.get("dev-a");

    finishOldGreeting({});
    await flush();

    expect(oldSession.close).toHaveBeenCalled();
    expect(oldLink.close).toHaveBeenCalled();
    expect(contextFor("dev-a")).toBe(freshContext);
    expect(freshContext.session).toBe(freshSession);
    expect(freshSession.close).not.toHaveBeenCalled();
    expect(freshLink.close).not.toHaveBeenCalled();
  });

  it("cancels a mint on reset before it can create or land a context", async () => {
    let finishOldMint;
    wire.openSession.mockImplementationOnce((options) => new Promise((resolve) => {
      finishOldMint = () => resolve(fakeSession(options.deviceId, options.onLost));
    }));
    const oldConnection = connectDevice("dev-a");
    await flush();

    resetApplication();
    await expect(oldConnection).rejects.toThrow(/retired|cancelled/);
    const freshContext = await connectDevice("dev-a");
    const freshSession = freshContext.session;
    const freshLink = linksFor.get("dev-a");

    finishOldMint();
    await flush();
    const staleSession = lastSession("dev-a");

    expect(staleSession).not.toBe(freshSession);
    expect(staleSession.close).toHaveBeenCalled();
    expect(contextFor("dev-a")).toBe(freshContext);
    expect(freshSession.close).not.toHaveBeenCalled();
    expect(freshLink.close).not.toHaveBeenCalled();
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
    expect(contextFor("dev-b").blocked).toBe("refused");
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
    await vi.waitFor(() => expect(document.querySelector(".device-picker-menu").hidden).toBe(false));

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
    await paintFeed();
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
    await paintFeed();
    await projectsOffered(["dev-a repo"]);

    rememberSelectedDevice("dev-b");
    syncHome();

    await projectsOffered(["dev-b repo"]);
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

  it("keeps retrying an online machine without waiting for another presence transition", async () => {
    unlinkable.add("dev-a");
    unlinkable.add("dev-b");
    await connectEveryDevice();
    expect(held()).toBe(false);
    expect(contextFor("dev-a").blocked).toBe("timeout");
    account.fetchDevices.mockClear();

    unlinkable.clear();
    await reachNextRecoveryAttempt();

    expect(held()).toBe(false);
    expect(openedFor("dev-a").length).toBeGreaterThan(1);
    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
    expect(account.fetchDevices).not.toHaveBeenCalled();
  });

  // Presence is the api's while the app is open (rule 6), and the app being
  // open is what the gate says: it starts the poll when it hands the page back
  // and stops it when it takes the page again.
  it("follows the account's presence on the app's own cadence while the app is open", async () => {
    await connectEveryDevice();
    account.fetchDevices.mockClear();

    // Not the gate's three seconds: a page that is standing on a machine reads
    // the account far less often than one that is waiting for one.
    await vi.advanceTimersByTimeAsync(3000);
    expect(account.fetchDevices).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(12000);
    expect(account.fetchDevices).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(15000);
    expect(account.fetchDevices).toHaveBeenCalledTimes(2);
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

    unlinkable.add("dev-a");
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

  // The api derives presence from a heartbeat, so a machine whose bridge has
  // just come back is listed offline for up to a minute and a half — over the
  // whole of a Retry's handshake. Standing that machine down mid-dial closes
  // the very rendezvous the dial is negotiating over, and the machine that was
  // about to answer is re-blocked with a reason that was never true.
  it("leaves a machine with a connect in flight alone when the account calls it offline", async () => {
    await connectEveryDevice();
    unlinkable.add("dev-b");
    await loseTheLink("dev-b");
    expect(contextFor("dev-b").blocked).toBe("timeout");

    unlinkable.delete("dev-b");
    slowMs.set("dev-b", 20);
    const retry = connectDevice("dev-b");
    await flush();
    rendezvousFor.get("dev-b").close.mockClear();

    devices = [online("dev-a", "Laptop"), away("dev-b", "Desktop")];
    App.devices = devices;
    deviceWentAway("dev-b");

    expect(rendezvousFor.get("dev-b").close).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(20);
    await retry;
    await flush();

    expect(liveIds()).toEqual(["dev-a", "dev-b"]);
  });

  // The churn Zech watched all day: the terminals' liveness probe timed out
  // behind a loaded bridge, closed the channel it had judged, and this layer
  // read that as the machine going — re-minting session, link and rendezvous
  // every few seconds. A terminal stream is one stream on the path, and the
  // path is the app channel's to report.
  it("keeps the device connected when the terminal channel goes alone", async () => {
    await connectEveryDevice();
    const link = linksFor.get("dev-a");

    const session = lastSession("dev-a");

    link.term.drop();
    await flush();

    expect(liveIds()).toContain("dev-a");
    expect(contextFor("dev-a").offline).toBe(false);
    expect(lastSession("dev-a"), "the app session is the one it always was").toBe(session);
    expect(session.close, "nothing was re-minted").not.toHaveBeenCalled();
    expect(link.close, "the peer is left carrying").not.toHaveBeenCalled();
  });

  // And the judgement that IS about the path still costs the connection.
  // #60: the terminals ride one channel and judge one channel. This used to
  // escalate straight to losing the device, and a phone's healthy direct session
  // was closed at 49 s because the terminal probe timed out while the app channel
  // was carrying fine. The verdict is now the app session's own probe.
  it("keeps the device when the terminal channel says nothing was carrying but the app path answers", async () => {
    await connectEveryDevice();
    const link = linksFor.get("dev-a");
    const before = lastSession("dev-a");
    before.probePath = vi.fn(async () => "alive");

    link.term.drop("liveness-timeout");
    await flush();

    // The shells are re-established and the session is untouched: the terminals
    // were wrong about a channel they cannot see.
    expect(before.probePath).toHaveBeenCalledWith("term.liveness");
    expect(lastSession("dev-a")).toBe(before);
    expect(before.close).not.toHaveBeenCalled();
    expect(contextFor("dev-a").offline).toBe(false);
  });

  it("loses the device when the terminal channel says nothing was carrying and the app probe agrees", async () => {
    await connectEveryDevice();
    const link = linksFor.get("dev-a");
    const before = lastSession("dev-a");
    // The real probe severs the session itself on a dead verdict, which is what
    // ends the connection — and it logs the verdict on its way, which is what
    // makes this the only way a live session is closed from the client.
    before.probePath = vi.fn(async () => {
      // The real probe severs the session itself on a dead verdict; handing the
      // carrier back is how this fake reports that loss.
      before.peer(null);
      return "dead";
    });

    link.term.drop("liveness-timeout");
    await flush();

    expect(before.probePath).toHaveBeenCalledWith("term.liveness");
    expect(lastSession("dev-a")).not.toBe(before);
  });

  it("does not close a replacement rendezvous when the established link is lost mid-dial", async () => {
    await connectEveryDevice();
    const oldLink = linksFor.get("dev-a");
    slowMs.set("dev-a", 20);
    const replacement = connectDevice("dev-a");
    await flush();
    rendezvousFor.get("dev-a").close.mockClear();

    oldLink.app.drop();

    expect(rendezvousFor.get("dev-a").close).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(20);
    await replacement;
    expect(contextFor("dev-a").offline).toBe(false);
  });

  it("does not land or re-block a pending dial after the context registry resets", async () => {
    slowMs.set("dev-a", 20);
    const pending = connectDevice("dev-a");
    const rejected = expect(pending).rejects.toThrow(/cancelled/);
    await flush();

    resetDeviceContexts();
    await vi.advanceTimersByTimeAsync(20);

    await rejected;
    expect(contextFor("dev-a")).toBe(null);
    expect(wire.openPeerLink).not.toHaveBeenCalled();
  });

  it("closes the provisional session before a late peer after reset", async () => {
    const events = [];
    let releasePeer;
    wire.openPeerLink.mockImplementation(() => new Promise((resolve) => { releasePeer = resolve; }));
    const pending = connectDevice("dev-a");
    const rejected = expect(pending).rejects.toThrow(/cancelled/);
    await flush();
    const session = lastSession("dev-a");
    session.close.mockImplementation(() => events.push("session"));
    const link = fakePeerLink("dev-a");
    link.close.mockImplementation(() => events.push("peer"));

    resetDeviceContexts();
    releasePeer(link);
    await rejected;

    expect(events).toEqual(["session", "peer"]);
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

  it("ignores an old carrier callback while its replacement greeting is pending", async () => {
    await connectEveryDevice();
    const oldSession = lastSession("dev-a");
    await loseTheLink("dev-a");
    let releaseGreeting;
    greetings.set("dev-a", () => new Promise((resolve) => { releaseGreeting = resolve; }));

    const replacement = connectDevice("dev-a");
    await flush();
    const newSession = lastSession("dev-a");
    const hellosBefore = newSession.call.mock.calls.filter(([method]) => method === "session.hello").length;

    await oldSession.fireCarrier();

    expect(newSession.call.mock.calls.filter(([method]) => method === "session.hello")).toHaveLength(hellosBefore);
    releaseGreeting({ capabilities: {} });
    await replacement;
  });
});
