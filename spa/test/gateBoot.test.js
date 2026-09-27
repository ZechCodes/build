// @vitest-environment jsdom
// Boot paints from the cache, before the wire.
//
// A reload used to show the gate twice over: `GET /api/devices` had to answer,
// then an E2EE session had to open, and only then did anything the reader
// recognises appear. Everything on that first screen is already on disk, so
// the disk is what paints it — and the network path that follows never takes
// a painted shell away again. It marks the device picker instead.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { fakeSession } from "./deviceSessionFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

// What the account answers with, and how long it takes about it. A boot that
// paints from the cache does not wait for this at all, which is what the
// never-answering default is here to prove.
const account = vi.hoisted(() => ({
  fetchDevices: vi.fn(() => new Promise(() => {})),
  openSession: vi.fn(() => new Promise(() => {})),
  startCacheSync: vi.fn(),
}));

vi.mock("../src/api.js", async (importOriginal) => ({
  ...(await importOriginal()),
  fetchDevices: (...args) => account.fetchDevices(...args),
  fetchDownloads: async () => ({ platforms: [], version: "1.0.0" }),
  mintInstallCommand: async () => ({ install_command: "curl … | sh", expires_in_s: 600 }),
  lookupDevice: async () => ({ name: "studio", fingerprint: "AAAA" }),
  approveDevice: async () => {},
}));

// connection.js is the wire; this suite is about the page before the wire says
// anything. Nothing here opens a session, so no context ever lands.
vi.mock("../src/connection.js", () => ({
  openDeviceSessions: (...args) => ({
    first: account.openSession(...args),
    settled: Promise.resolve([]),
  }),
  securityStopText: () => "",
  connectDevice: async () => null,
  deviceWentAway: () => {},
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {},
  syncHome: () => {},
  goOffline: () => {},
  retireDevice: () => {},
  chooseCreationDevice: () => {},
  openDeviceSettingsSession: async () => ({}),
  deviceRecoverySnapshot: () => [],
  onDeviceRecoveryChanged: () => () => {},
  forgetHomeFollow: () => {},
  forgetRendezvousSockets: () => {},
  forgetSecurityStops: () => {},
}));

// The sync layer's own suite owns what a pass reads (cacheSync.test.js); what
// matters here is only when it is started.
vi.mock("../src/core/cacheSync.js", () => ({
  startCacheSync: (...args) => account.startCacheSync(...args),
  stopCacheSync: () => {},
  routeChanged: () => {},
}));

let App;
let boot;
let holdAppWhileNoDeviceAnswers;
let writeCached;
let DEVICES_ADDRESS;
let stopFeed;

const device = (id, status = "online") => ({ id, name: id, status, fingerprint: `${id}-fingerprint` });

/** A workspace on `dev-1`, as the last pass left it on disk. */
const cachedWorkspace = {
  id: "ws-1",
  workspace_id: "ws-1",
  project_id: "proj-1",
  name: "Checkout",
  status: "ready",
  deviceId: "dev-1",
  projectKey: "dev-1/proj-1",
  workspaceKey: "dev-1/ws-1",
};

const cachedProject = { id: "proj-1", project_id: "proj-1", name: "Payments", deviceId: "dev-1", projectKey: "dev-1/proj-1" };

/** Everything a boot paint reads: the account's machines, and one machine's
 *  board with its two lists. */
async function warmCache(devices = [device("dev-1")]) {
  await writeCached(DEVICES_ADDRESS, devices);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, {
    items: [],
    plans: [],
    runs: [],
    externalWorktrees: [],
    pending: [],
    projects: [cachedProject],
    workspaces: [cachedWorkspace],
  });
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, [cachedProject]);
  await writeCached({ deviceId: "dev-1", entityId: "", kind: "workspaces" }, [cachedWorkspace]);
}

/** A turn of the event loop, which is what the cache answers on. */
const tick = () => new Promise((done) => setTimeout(done, 5));

// Every project also has its project agent's row (#103); these tests are
// about the workspace and capture rows.
const railEntries = () => [...document.querySelectorAll("#inbox-list .inbox-entry:not(.inbox-project-agent)")].map((row) => row.dataset.key);
const unreachableMark = () => document.querySelector(".device-picker-unreachable");

beforeEach(async () => {
  vi.resetModules();
  vi.clearAllMocks();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  localStorage.clear();
  document.body.className = "";
  document.body.innerHTML = bodyHtml;
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });
  account.fetchDevices.mockImplementation(() => new Promise(() => {}));
  account.openSession.mockImplementation(() => new Promise(() => {}));
  ({ App } = await import("../src/app.js"));
  ({ writeCached, DEVICES_ADDRESS } = await import("../src/core/localCache.js"));
  ({ stopFeed } = await import("../src/core/taskFeed.js"));
  Object.assign(App, { devices: [], gated: true, route: { name: "inbox" }, _connecting: false, _watch: null });
  ({ boot, holdAppWhileNoDeviceAnswers } = await import("../src/views/gate.js"));
});

afterEach(async () => {
  clearInterval(App._watch);
  App._watch = null;
  stopFeed();
  // The rail a boot mounted outlives vi.resetModules, and still hears the next
  // test's cache writes the way another tab would; unmounted, it paints nothing
  // into that test's page.
  (await import("../src/core/inboxView.js")).unmountInboxList();
});

describe("booting from the cache", () => {
  it("paints the shell and the rail's rows before the account list answers", async () => {
    await warmCache();

    void boot();

    await vi.waitFor(() => expect(railEntries()).toEqual(["workspace:dev-1/ws-1"]));
    expect(App.gated).toBe(false);
    expect(document.body.classList.contains("gated")).toBe(false);
    expect(document.getElementById("devpick").hidden).toBe(false);
    // The account list is still in flight — nothing here waited on it.
    expect(App.devices.map((each) => each.id)).toEqual(["dev-1"]);
  });

  it("gates as it always has when the cache has nothing to paint", async () => {
    account.fetchDevices.mockResolvedValue([]);

    await boot();

    expect(App.gated).toBe(true);
    expect(document.body.classList.contains("gated")).toBe(true);
    expect(document.getElementById("root").textContent).toContain("Welcome to Build");
  });

  it("marks the picker rather than taking a painted shell away when nothing answers", async () => {
    await warmCache([device("dev-1", "offline")]);
    account.fetchDevices.mockResolvedValue([device("dev-1", "offline")]);
    account.openSession.mockRejectedValue(new Error("no device answered"));

    await boot();

    expect(App.gated).toBe(false);
    expect(document.getElementById("waitlist")).toBeNull();
    expect(railEntries()).toEqual(["workspace:dev-1/ws-1"]);
    expect(unreachableMark()).not.toBeNull();
  });

  it("keeps the painted shell when the account list itself cannot be read", async () => {
    await warmCache([device("dev-1", "offline")]);
    account.fetchDevices.mockRejectedValue(new Error("api unreachable"));

    await boot();

    expect(App.gated).toBe(false);
    expect(document.getElementById("root").textContent).not.toContain("Loading your devices");
    expect(unreachableMark()).not.toBeNull();
  });

  // The reload the whole stage is about: the machine is down, so skriftapp
  // cannot be reached either and the cached rows still say what the last
  // successful read said — online. The shell stays, but a shell with nothing
  // saying so and nothing that will ever reconnect is the screenshot again.
  it("marks the picker and keeps watching when the account list fails and the cache says online", async () => {
    await warmCache([device("dev-1", "online")]);
    account.fetchDevices.mockRejectedValue(new Error("api unreachable"));

    await boot();

    expect(App.gated).toBe(false);
    expect(railEntries()).toEqual(["workspace:dev-1/ws-1"]);
    expect(unreachableMark()).not.toBeNull();
    // The gate's own three seconds: the one thing that re-enters the app when
    // a machine comes back, now that the painted app has no Retry button.
    expect(App._watch).not.toBeNull();
  });

  // The one screen a cached board cannot stand in for: a bridge answering in a
  // shape this tab cannot read. The gate's own watch re-boots every three
  // seconds while nothing answers, and a boot that repainted the app over that
  // screen would take away the only thing saying why the app cannot be used.
  it("leaves a version gate on the page rather than painting the cache over it", async () => {
    await warmCache();
    App.devices = [device("dev-1")];
    const { adoptBridgeSelection, adoptDeviceSession, contextFor } = await import("../src/core/deviceContexts.js");
    adoptDeviceSession(fakeSession("dev-1"));
    adoptBridgeSelection(contextFor("dev-1"), { unsupported: "app", version: "3.0.0" });
    holdAppWhileNoDeviceAnswers();
    await vi.waitFor(() => expect(document.querySelector("#root h1")).not.toBeNull());
    const heading = document.querySelector("#root h1").textContent;

    void boot();
    await tick();
    await tick();

    expect(App.gated).toBe(true);
    expect(document.querySelector("#root h1")?.textContent).toBe(heading);
    expect(railEntries()).toEqual([]);
  });

  it("starts the sync layer before the first session answers", async () => {
    account.fetchDevices.mockResolvedValue([device("dev-1")]);

    void boot();

    await vi.waitFor(() => expect(account.startCacheSync).toHaveBeenCalledTimes(1));
    // Still opening: the sync layer is up before anything has answered it.
    expect(App.gated).toBe(true);
  });
});
