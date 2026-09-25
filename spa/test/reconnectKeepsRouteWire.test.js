// @vitest-environment jsdom
// #170: a reconnect leaves the mounted route in place.
//
// The page a reader is standing on was painted from the cache, and a session
// landing under it — the first one after a cold reload, or a new one after the
// machine went away and came back — is news about the wire, not about the
// page. So the route is not torn down and built again: the same nodes stay in
// #root, with the reader's focus in them, and nothing the view reads on its
// first mount is read again. It repaints only when a cache write announces.
//
// Three things still take the page and build it again, and each has its
// control here: an account change, the route's machine being retired, and the
// app-behind version gate.
//
// Nothing between the wire and the page is a stand-in: the real gate
// (views/gate.js), router and views (app.js), device registry and connection
// layer (connection.js), sync layer and cache over fake-indexeddb. What is
// stood in for is only what leaves this tab: the relay rendezvous, the E2EE
// session it mints and the direct connection it negotiates, and the account's
// REST list.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const DEVICE = "dev-1";
const ROUTE_HASH = `#/device/${DEVICE}/project/proj-1/workspaces`;

const wire = vi.hoisted(() => ({
  // What the account lists, and each session this tab was handed, newest last.
  listed: [],
  sessions: [],
  // What the next session answers `session.hello` with.
  greeting: {},
  // Held open until a case lets the session land.
  landing: null,
}));

vi.mock("../src/api.js", async (importOriginal) => ({
  ...(await importOriginal()),
  fetchDevices: async () => wire.listed,
  fetchGatewayToken: async () => "tok",
  fetchIceServers: async () => [],
}));

vi.mock("../src/core/rendezvous.js", () => ({
  createRelayRendezvous: ({ deviceId }) => {
    const rendezvous = {
      deviceId,
      opened: true,
      open: async () => {},
      mint: async () => ({ sessionId: `sess-${deviceId}`, sessionKeyB64: "key", deviceId }),
      signalCarrier: () => ({ onClose: () => {}, close: () => {} }),
      isOpen: () => rendezvous.opened,
      onClosed: () => () => {},
      close: () => {
        rendezvous.opened = false;
      },
    };
    return rendezvous;
  },
}));

vi.mock("../src/core/session.js", () => ({
  openSession: async ({ deviceId }) => {
    await wire.landing;
    const session = bridgeSession(deviceId);
    wire.sessions.push(session);
    return session;
  },
}));

vi.mock("../src/core/peerLink.js", () => ({
  openPeerLink: async () => ({
    app: { onClose: () => {} },
    term: { onClose: () => {} },
    recovery: { snapshot: () => ({ epoch: 0, recovering: false }), subscribe: () => () => {} },
    onPathChanged: () => () => {},
    onRestored: () => () => {},
    transportPath: () => null,
    close: () => {},
  }),
}));

const cachedProject = { id: "proj-1", project_id: "proj-1", name: "Payments", deviceId: DEVICE, projectKey: `${DEVICE}/proj-1` };
const cachedWorkspace = {
  id: "ws-1",
  workspace_id: "ws-1",
  project_id: "proj-1",
  name: "Checkout",
  status: "ready",
  deviceId: DEVICE,
  projectKey: `${DEVICE}/proj-1`,
  workspaceKey: `${DEVICE}/ws-1`,
};

/** The machine's bridge, answering with what the cache already holds of it. */
function bridgeSession(deviceId) {
  const greeting = wire.greeting;
  const answers = {
    "session.hello": () => greeting,
    "project.list": () => ({ projects: [{ project_id: "proj-1", name: "Payments" }] }),
    "workspace.list": () => ({ workspaces: [{ workspace_id: "ws-1", project_id: "proj-1", title: "Checkout", state: "ready" }] }),
    "board.list": () => ({ items: [] }),
  };
  // Riding a carrier is what greets the bridge (connection.js landSession).
  let carrierChanged = () => {};
  const session = {
    deviceId,
    sessionId: `session-${deviceId}-${wire.sessions.length + 1}`,
    call: vi.fn(async (method) => (answers[method] ? answers[method]() : {})),
    peer: (carrier) => (carrier ? carrierChanged() : undefined),
    fail: () => {},
    probePath: async () => "alive",
    installAdapter: (selection) => selection?.create?.(session.call) || null,
    adapter: () => null,
    onPush: () => () => {},
    onCarrier: (fn) => {
      carrierChanged = fn;
    },
    watchRecovery: () => {},
    reattachSignaling: async () => {},
    confirmCarried: async () => "carried",
    closed: false,
    close: () => {
      session.closed = true;
    },
  };
  return session;
}

const online = { id: DEVICE, name: "studio", status: "online", fingerprint: "dev-1-fingerprint", last_seen_at: new Date().toISOString() };
const offline = { ...online, status: "offline" };

let modules;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  globalThis.RTCPeerConnection = function RTCPeerConnectionStub() {};
  localStorage.clear();
  document.body.className = "";
  document.body.innerHTML = bodyHtml;
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });
  history.replaceState(null, "", ROUTE_HASH);
  Object.assign(wire, { listed: [online], sessions: [], greeting: {}, landing: null });
  const app = await import("../src/app.js");
  modules = {
    app,
    App: app.App,
    cache: await import("../src/core/localCache.js"),
    connection: await import("../src/connection.js"),
    contexts: await import("../src/core/deviceContexts.js"),
    devices: await import("../src/devices.js"),
    gate: await import("../src/views/gate.js"),
    feed: await import("../src/core/taskFeed.js"),
    sync: await import("../src/core/cacheSync.js"),
  };
});

afterEach(async () => {
  if (!modules) return;
  const { App, devices, feed, sync, connection } = modules;
  clearInterval(App._watch);
  App._watch = null;
  devices.stopWatchingPresence();
  connection.stopWatchingForWake();
  sync.stopCacheSync();
  feed.stopFeed();
  (await import("../src/core/inboxView.js")).unmountInboxList();
  delete globalThis.RTCPeerConnection;
});

/** Everything the page paints from, as the last session left it on disk. */
async function warmCache() {
  const { writeCached, DEVICES_ADDRESS } = modules.cache;
  await writeCached(DEVICES_ADDRESS, [online]);
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "feed" }, {
    items: [],
    plans: [],
    runs: [],
    externalWorktrees: [],
    pending: [],
    projects: [cachedProject],
    workspaces: [cachedWorkspace],
  });
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "projects" }, [cachedProject]);
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "workspaces" }, [cachedWorkspace]);
}

/** The page the route mounted, and the one control in it the reader is on. */
const mountedPage = () => document.getElementById("tabbody");
const workspaceRow = () => document.querySelector('#project-pane [data-workspace="dev-1/ws-1"]');
const filterButton = () => document.querySelector('#project-pane [data-workspace-filter="all"]');

/** The page as the reader has it: painted, with their focus in it, and a watch
 *  on the view's teardown — which is what a remount runs first. */
function standOnPage() {
  const { App } = modules;
  const page = mountedPage();
  const focused = filterButton();
  focused.focus();
  const dispose = App.viewDispose;
  const teardown = vi.fn(() => dispose?.());
  App.viewDispose = teardown;
  return { page, focused, teardown };
}

/** A reload onto the route: the router reads the URL, the cache paints the
 *  page, and the session to the machine is still being dialled. */
async function reloadOntoRoute() {
  const { app, devices, gate } = modules;
  let land;
  wire.landing = new Promise((done) => {
    land = done;
  });
  await warmCache();
  app.initRouter();
  devices.initDevicePicker();
  const booted = gate.boot();
  await vi.waitFor(() => expect(workspaceRow()).not.toBeNull());
  expect(wire.sessions).toHaveLength(0);
  return { land, booted };
}

/** A reload that has finished coming up: the session has landed and the gate
 *  is listening for the account running out of machines. */
async function connectedOnRoute() {
  const { land, booted } = await reloadOntoRoute();
  land();
  await booted;
  await vi.waitFor(() => expect(modules.contexts.liveContexts()).toHaveLength(1));
  await settle();
}

/** The machine goes away and comes back as the account says so, and a new
 *  session to it lands. */
async function replaceSessionThroughPresence() {
  const { contexts, devices } = modules;
  const before = wire.sessions.length;
  wire.listed = [offline];
  await devices.readPresence();
  await vi.waitFor(() => expect(contexts.liveContexts()).toHaveLength(0));
  wire.listed = [online];
  await devices.readPresence();
  await vi.waitFor(() => expect(contexts.liveContexts()).toHaveLength(1));
  expect(wire.sessions).toHaveLength(before + 1);
}

const settle = async () => {
  for (let turn = 0; turn < 20; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

it("keeps the cache-painted route when the first session lands under it", async () => {
  const { land, booted } = await reloadOntoRoute();
  const { page, focused, teardown } = standOnPage();

  land();
  await booted;
  await vi.waitFor(() => expect(modules.contexts.liveContexts()).toHaveLength(1));
  await settle();

  expect(mountedPage()).toBe(page);
  expect(page.isConnected).toBe(true);
  expect(document.activeElement).toBe(focused);
  expect(teardown).not.toHaveBeenCalled();
  // Every mount installs its own teardown, and the first-mount reads come with
  // it: the one still installed is the one the reader's page was built with.
  expect(modules.App.viewDispose).toBe(teardown);
});

it("keeps the route when the machine goes away and a new session replaces the old one", async () => {
  await connectedOnRoute();
  const replaced = wire.sessions.at(-1);
  const { page, focused, teardown } = standOnPage();

  await replaceSessionThroughPresence();
  await settle();

  expect(replaced.closed).toBe(true);
  expect(modules.contexts.contextFor(DEVICE).session).toBe(wire.sessions.at(-1));
  expect(mountedPage()).toBe(page);
  expect(document.activeElement).toBe(focused);
  expect(teardown).not.toHaveBeenCalled();
  // Every mount installs its own teardown, and the first-mount reads come with
  // it: the one still installed is the one the reader's page was built with.
  expect(modules.App.viewDispose).toBe(teardown);
  expect(workspaceRow()).not.toBeNull();
});

// ---- the genuine remounts ---------------------------------------------------

it("builds the route again after an account change", async () => {
  await connectedOnRoute();
  const { page, teardown } = standOnPage();

  // The next account's arrival: everything of the last one goes, and the boot
  // that follows stands the reader on the route again.
  modules.app.resetApplication();
  await warmCache();
  await modules.gate.boot();
  await vi.waitFor(() => expect(modules.contexts.liveContexts()).toHaveLength(1));
  await settle();

  expect(mountedPage()).not.toBe(page);
  expect(mountedPage()).not.toBeNull();
  expect(teardown).toHaveBeenCalled();
});

it("builds the route again when its machine was retired and lands anew", async () => {
  await connectedOnRoute();
  const { page, teardown } = standOnPage();

  modules.connection.retireDevice(DEVICE);
  await settle();
  modules.connection.openDeviceSessions();
  await vi.waitFor(() => expect(modules.contexts.liveContexts()).toHaveLength(1));
  await settle();

  expect(mountedPage()).not.toBe(page);
  expect(mountedPage()).not.toBeNull();
  expect(teardown).toHaveBeenCalled();
});

it("builds the route again when the app-behind version gate lets the app back in", async () => {
  await connectedOnRoute();
  const { page, teardown } = standOnPage();

  // The machine's bridge was updated past this build: the next session greets
  // with a major nothing here speaks, and the gate takes the page.
  wire.greeting = { api_version: "2.0.0" };
  modules.connection.connectDevice(DEVICE).catch(() => {});
  await vi.waitFor(() => expect(document.querySelector("#root h1")?.textContent).toContain("This app is behind the bridge"));
  expect(teardown).toHaveBeenCalled();

  // This tab is updated to meet it (a greeting an adapter here claims), and
  // the reader gets their route back — built again, since the gate had #root.
  wire.greeting = {};
  modules.connection.connectDevice(DEVICE).catch(() => {});
  await vi.waitFor(() => expect(mountedPage()).not.toBeNull());
  await settle();

  expect(modules.App.gated).toBe(false);
  expect(mountedPage()).not.toBe(page);
  expect(workspaceRow()).not.toBeNull();
});
