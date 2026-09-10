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
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { host, opts, dispose() {} };
  },
}));

const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3" }];
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn({ items: feedItems, projects: [] });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: () => {},
  primaryRunIdFor: () => null,
}));

const { App } = await import("../src/app.js");
const { setCacheDevice } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const region = () => document.getElementById("console-region");
const bar = () => region().querySelector(".console-bar");
const tabs = () => [...region().querySelectorAll(".console-tab-name")].map((cell) => cell.textContent);

let panel = null;

const mountAndOpen = async () => {
  panel = mountConsole(region(), { kind: "branch", projectId: "p1", branch: "build/login" });
  await flush();
  bar().click();
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetConsoleMemory();
  setCacheDevice("dev-1");
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
