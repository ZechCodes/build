// @vitest-environment jsdom
// #171: a machine retired while the agent rail's first catalog read is out.
//
// The rail stands up over the route's machine and asks it two things before it
// seeds its new-agent choice: the harnesses it offers (`models.list`) and what
// a project agent starts on (`settings.get`). A retirement landing while those
// are out is an expected outcome, not an error: the reads resolve to nothing,
// the rail over the retired machine paints no more, and when the machine lands
// anew the route's rail is stood up again over it and paints what the cache
// holds. Nothing rejects unhandled on the way.
//
// Nothing between the wire and the rail is a stand-in: the real gate
// (views/gate.js), router, shell and rail (app.js, core/shell.js,
// core/agentRail.js), device registry and connection layer (connection.js),
// sync layer and cache over fake-indexeddb. What is stood in for is only what
// leaves this tab: the relay rendezvous, the E2EE session it mints and the
// direct connection it negotiates, and the account's REST list.

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
  // Methods the next answer to is held until the case lets it go, and a
  // listener told the moment one of them is asked.
  held: new Map(),
  asked: () => {},
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

// The project's list names no owner for its conversation, so the rail goes up
// once the machine answers `project.ensure_conversation` — over a session that
// has greeted, which is when its first catalog read goes out on the wire.
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

/** The machine's bridge. An answer the case is holding waits for its release. */
function bridgeSession(deviceId) {
  const answers = {
    "session.hello": () => ({}),
    "project.list": () => ({ projects: [{ project_id: "proj-1", name: "Payments" }] }),
    "workspace.list": () => ({ workspaces: [{ workspace_id: "ws-1", project_id: "proj-1", title: "Checkout", state: "ready" }] }),
    "board.list": () => ({ items: [] }),
    "models.list": () => ({ models: [], efforts: [] }),
    "settings.get": () => ({}),
  };
  // Riding a carrier is what greets the bridge (connection.js landSession).
  let carrierChanged = () => {};
  const session = {
    deviceId,
    sessionId: `session-${deviceId}-${wire.sessions.length + 1}`,
    call: vi.fn(async (method) => {
      const held = wire.held.get(method);
      if (held) {
        wire.asked(method);
        await held.promise;
      }
      return answers[method] ? answers[method]() : {};
    }),
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

/** Hold the next answers to these methods until `release()`. */
function hold(...methods) {
  for (const method of methods) {
    let release;
    const promise = new Promise((done) => {
      release = done;
    });
    wire.held.set(method, { promise, release });
  }
  return () => {
    for (const method of methods) wire.held.get(method)?.release();
    wire.held.clear();
  };
}

/** Resolves the moment the bridge is asked `method` (a held one). */
function whenAsked(method) {
  return new Promise((done) => {
    wire.asked = (asked) => {
      if (asked === method) done();
    };
  });
}

const online = { id: DEVICE, name: "studio", status: "online", fingerprint: "dev-1-fingerprint", last_seen_at: new Date().toISOString() };

let modules;
let unhandled;
const noteUnhandled = (reason) => unhandled.push(reason);

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
  Object.assign(wire, { listed: [online], sessions: [], held: new Map(), asked: () => {} });
  unhandled = [];
  process.on("unhandledRejection", noteUnhandled);
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
  process.off("unhandledRejection", noteUnhandled);
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

const railStrip = () => document.querySelector("#agent-rail .rail-strip");
const askedOf = (session, method) => session.call.mock.calls.filter(([asked]) => asked === method).length;

const settle = async () => {
  for (let turn = 0; turn < 20; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

it("resolves the rail's first catalog read to nothing when its machine is retired under it", async () => {
  const { app, connection, contexts, devices, gate } = modules;
  // The rail's first catalog read — the harnesses and the project agent's
  // setting — is held on the wire from the moment the rail asks it.
  const release = hold("models.list", "settings.get");
  const asked = whenAsked("settings.get");
  await warmCache();
  app.initRouter();
  devices.initDevicePicker();
  await gate.boot();
  await asked;
  const retiredSession = wire.sessions.at(-1);
  const retiredStrip = railStrip();
  expect(retiredStrip).not.toBeNull();

  // The account drops the machine while the read is out, and then the read
  // comes back.
  connection.retireDevice(DEVICE);
  release();
  await settle();

  expect(unhandled).toEqual([]);
  expect(retiredSession.closed).toBe(true);

  // The machine lands anew: the route's rail is stood up again over it, from
  // what the cache holds, and asks the machine now standing for its catalog.
  connection.openDeviceSessions();
  await vi.waitFor(() => expect(contexts.liveContexts()).toHaveLength(1));
  const landed = wire.sessions.at(-1);
  expect(landed).not.toBe(retiredSession);
  await vi.waitFor(() => expect(askedOf(landed, "settings.get")).toBe(1));
  await settle();

  expect(retiredStrip.isConnected).toBe(false);
  expect(railStrip()).not.toBeNull();
  expect(document.querySelector("#agent-rail #rail-panel")).not.toBeNull();
  expect(unhandled).toEqual([]);
});
