// @vitest-environment jsdom
// The console against the local cache. The tab list is the `terminals` record
// the sync layer writes, the checkout the shells run in comes off the cached
// row, and nothing here asks the machine anything to paint: no `branch.get`,
// no `term.list`. A push that lands while the console is open moves the strip.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

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
  terminalDeviceId: () => null,
  followTerminalDevice: () => {},
  resetTerminalManager: () => {},
  provideTerminalSessions: () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { host, opts, dispose() {} };
  },
}));

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

const branchRow = (deviceId = "dev-1", over = {}) => ({
  kind: "branch",
  deviceId,
  project_id: "p1",
  projectKey: `${deviceId}:p1`,
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  ...over,
});

/** One device's world as a sync pass leaves it: the two lists, one row, and
 *  whatever shells that row's checkout is holding. */
const seedDevice = async (deviceId, { row = branchRow(deviceId), tabs: termTabs = [] } = {}) => {
  await writeCached({ deviceId, entityId: "", kind: "projects" }, []);
  await writeCached({ deviceId, entityId: "", kind: "workspaces" }, []);
  const entityId = row.run_id;
  await writeCached({ deviceId, entityId, kind: "row" }, row);
  await writeCached({ deviceId, entityId, kind: "terminals" }, { tabs: termTabs });
};

const mountAndOpen = async (deviceId = "dev-1") => {
  panel = mountConsole(region(), {
    kind: "branch",
    deviceId,
    projectId: "p1",
    branch: "build/login",
    call: (...args) => bridge.call(...args),
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
  await wipeCache();
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "term-9" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  manager.attachTerminal.mockReset().mockResolvedValue({ snapshot: "", cursor: 0 });
  bridge.call = vi.fn(async () => ({}));
});

afterEach(() => {
  if (panel) panel.dispose();
  panel = null;
});

describe("the tab strip", () => {
  it("is the terminals record, painted without asking the machine anything", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }, { term_id: "term-2" }] });
    await mountAndOpen();
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    expect(bridge.call).not.toHaveBeenCalled();
    expect(manager.listTerminals).not.toHaveBeenCalled();
  });

  it("takes the checkout the shells run in off the cached row", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }] });
    await mountAndOpen();
    expect(manager.attachTerminal).toHaveBeenCalledWith("term-1", { run_id: "run-3" }, expect.anything());
  });

  it("grows a tab when a push announces one", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }] });
    await mountAndOpen();
    expect(tabs()).toEqual(["Terminal 1"]);

    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "terminals" },
      { tabs: [{ term_id: "term-1" }, { term_id: "term-7" }] },
    );
    await flush();

    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
  });

  it("drops a tab the push stopped naming", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }, { term_id: "term-2" }] });
    await mountAndOpen();

    await writeCached({ deviceId: "dev-1", entityId: "run-3", kind: "terminals" }, { tabs: [{ term_id: "term-2" }] });
    await flush();

    expect(tabs()).toEqual(["Terminal 1"]);
  });

  it("says there is nowhere to open a shell when no row answers to the route", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, []);
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "workspaces" }, []);
    await mountAndOpen();
    expect(region().textContent).toContain("no checkout here");
    expect(bridge.call).not.toHaveBeenCalled();
  });

  it("stands the console up when the row it is waiting for lands", async () => {
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "projects" }, []);
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "workspaces" }, []);
    await mountAndOpen();
    expect(tabs()).toEqual([]);

    await seedDevice("dev-1", { tabs: [{ term_id: "term-4" }] });
    await flush();

    expect(tabs()).toEqual(["Terminal 1"]);
  });
});

describe("opening and closing one", () => {
  it("writes the new terminal into the record the strip reads", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }] });
    await mountAndOpen();
    region().querySelector(".console-new").click();
    await flush();
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "terminals" });
    expect(record.value.tabs.map((tab) => tab.term_id)).toEqual(["term-1", "term-9"]);
  });

  it("takes a closed terminal out of the record too", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }, { term_id: "term-2" }] });
    await mountAndOpen();
    region().querySelectorAll(".console-tab .tx")[0].click();
    await flush();
    expect(manager.closeTerminal).toHaveBeenCalledWith("term-1");
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "terminals" });
    expect(record.value.tabs.map((tab) => tab.term_id)).toEqual(["term-2"]);
  });

  it("remembers which tab was open in the console record", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }, { term_id: "term-2" }] });
    await mountAndOpen();
    region().querySelectorAll(".console-tab-name")[1].click();
    await flush();
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "console" });
    expect(record.value).toEqual({ selected: "term-2" });
  });

  it("reopens on the tab the console record names", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-1" }, { term_id: "term-2" }] });
    await writeCached({ deviceId: "dev-1", entityId: "run-3", kind: "console" }, { selected: "term-2" });
    await mountAndOpen();
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-2");
  });
});

// The rail merges every device's rows, and every machine mints a `p1`. The
// console stands in one checkout on one machine — the one the link that opened
// the surface named — so the records it reads are that device's.
describe("an account with more than one device", () => {
  it("reads the route device's row and its terminals", async () => {
    await seedDevice("dev-1", { tabs: [{ term_id: "term-4" }, { term_id: "term-5" }] });
    await seedDevice("dev-2", {
      row: branchRow("dev-2", { run_id: "run-9", worktree_id: "wt-9" }),
      tabs: [{ term_id: "term-1" }],
    });

    await mountAndOpen("dev-2");

    expect(tabs()).toEqual(["Terminal 1"]);
    expect(manager.attachTerminal).toHaveBeenCalledWith("term-1", { run_id: "run-9" }, expect.anything());
  });
});
