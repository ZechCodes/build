// @vitest-environment jsdom
// The console's tab list against the local cache: a revisit renders the tabs
// without a round trip, the live list reconciles whatever drifted, and both
// the first listing and every create/close write through.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const manager = {
  listTerminals: vi.fn(async () => []),
  createTerminal: vi.fn(async () => ({ term_id: "term-9" })),
  closeTerminal: vi.fn(async () => {}),
  attachTerminal: vi.fn(async () => ({ snapshot: "", cursor: 0 })),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => manager,
  subscribeTerminalStatus: () => () => {},
  // Which machine the shells type at, and the moves between machines, are the
  // app spine's business and not this suite's: they answer, and nothing moves.
  terminalDeviceId: () => null,
  followTerminalDevice: () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { host, opts, dispose() {} };
  },
}));

const homeRow = { kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3", deviceId: "dev-1" };
let feedSnapshot = { items: [homeRow], projects: [] };
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn(feedSnapshot);
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: () => {},
  primaryRunIdFor: () => null,
  dropFeedDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const region = () => document.getElementById("console-region");
const bar = () => region().querySelector(".console-bar");
const tabs = () => [...region().querySelectorAll(".console-tab-name")].map((cell) => cell.textContent);

let panel = null;

const mountAndOpen = async (deviceId = "dev-1") => {
  panel = mountConsole(region(), {
    kind: "branch",
    deviceId,
    projectId: "p1",
    branch: "build/login",
    call: (...args) => App.call(...args),
    cacheScope: scopeFor(deviceId),
  });
  await flush();
  bar().click();
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetConsoleMemory();
  feedSnapshot = { items: [homeRow], projects: [] };
  scopeFor("dev-1"); // the machine these reads are addressed to
  await wipeCache();
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "term-9" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  manager.attachTerminal.mockReset().mockResolvedValue({ snapshot: "", cursor: 0 });
  App.call = vi.fn(async (method) => {
    if (method === "branch.get") return { project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3" };
    return {};
  });
});

afterEach(() => {
  if (panel) panel.dispose();
  panel = null;
});

describe("the cached tab list", () => {
  it("renders the saved tabs without waiting on the machine", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "tabs" },
      { scope: { run_id: "run-3" }, termIds: ["term-1", "term-2"] },
    );
    App.call = vi.fn(() => new Promise(() => {}));
    manager.listTerminals.mockImplementation(() => new Promise(() => {}));
    await mountAndOpen();
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
  });

  it("reconciles the saved tabs against the live list and writes it through", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "tabs" },
      { scope: { run_id: "run-3" }, termIds: ["term-1", "term-2"] },
    );
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mountAndOpen();
    expect(tabs()).toEqual(["Terminal 1"]);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "tabs" });
    expect(record.value.termIds).toEqual(["term-1"]);
  });

  it("writes the first-ever listing through for the next visit", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-5" }]);
    await mountAndOpen();
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "tabs" });
    expect(record.value).toEqual({ scope: { run_id: "run-3" }, termIds: ["term-5"] });
  });
});

// The rail merges every device's rows, and every machine mints a `p1`. The
// console stands in one checkout on one machine — the one the link that opened
// the surface named — so the row it addresses the tab cache with is that
// device's, not whichever `p1` the merge happens to list first.
describe("an account with more than one device", () => {
  it("addresses the tab cache with the route device's row", async () => {
    // The desktop's own build/login sorts first in the merge, and this console
    // is the desktop's: its run is what the saved tabs are filed under.
    const theirs = { kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-9", worktree_id: "wt-9", deviceId: "dev-2" };
    feedSnapshot = {
      items: [theirs, homeRow],
      projects: [],
      devices: { "dev-2": { items: [theirs], projects: [] }, "dev-1": { items: [homeRow], projects: [] } },
    };
    await writeCached(
      { deviceId: "dev-2", entityId: "run-9", kind: "tabs" },
      { scope: { run_id: "run-9" }, termIds: ["term-1"] },
    );
    // This machine's own row is cached too, under a different device and a
    // different run: reading it here would paint two tabs instead of one.
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "tabs" },
      { scope: { run_id: "run-3" }, termIds: ["term-4", "term-5"] },
    );
    App.call = vi.fn(() => new Promise(() => {}));
    manager.listTerminals.mockImplementation(() => new Promise(() => {}));

    await mountAndOpen("dev-2");

    expect(tabs()).toEqual(["Terminal 1"]);
  });
});
