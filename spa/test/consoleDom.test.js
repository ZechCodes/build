// @vitest-environment jsdom
// The console: the bar at the bottom of a work surface, the terminals of the
// checkout behind it, and the three sizes it opens to.
//
// Everything it draws is off the cache — the routed row says which checkout,
// the `terminals` record says which shells — so every case here puts that
// world on the disk and then mounts.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

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
  resetTerminalManager: () => {},
  // Minting a terminal session is the connection layer's (spec rule 5); no
  // suite here opens one.
  provideTerminalSessions: () => {},
}));

// No ghostty/wasm under node: the pane is a leaf that reports what it was asked
// to attach to.
const panes = [];
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    const pane = { host, opts, disposed: false, dispose() { pane.disposed = true; } };
    panes.push(pane);
    return pane;
  },
}));

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));

const { consoleBranchRow, consoleCacheScope, emptyConsoleWorld, seedConsoleTerminals, seedConsoleWorld } =
  await import("./consoleWorld.js");
const { App } = await import("../src/app.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");
const { consoleKey, markConsoleTerminal, takeConsoleTerminal } = await import("../src/core/consoleModel.js");
const { readCached, writeCached } = await import("../src/core/localCache.js");
const { uiAddress } = await import("../src/core/localUiState.js");

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const region = () => document.getElementById("console-region");
const bar = () => region().querySelector(".console-bar");
const tabs = () => [...region().querySelectorAll(".console-tab-name")].map((cell) => cell.textContent);
const size = () => region().dataset.size;
const callsTo = (method) => calls.filter((call) => call.method === method);

let calls = [];
let branchRow = null;
let panel = null;

const branchAddress = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  projectId: "p1",
  branch: "build/login",
  call: (...args) => bridge.call(...args),
  cacheScope: consoleCacheScope(over.deviceId || "dev-1"),
  ...over,
});
const sizeAddress = (context = branchAddress()) => uiAddress({
  deviceId: context.deviceId,
  entityId: consoleKey(context),
  view: "console",
  kind: "fold",
});

const mount = async (context = branchAddress()) => {
  panel = mountConsole(region(), context);
  await flush();
  return panel;
};

/** The world this suite's branch stands in, then the console over it. */
const mountOver = async (terminals, context = branchAddress()) => {
  await seedConsoleWorld({ terminals });
  return mount(context);
};

const open = async () => {
  bar().click();
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetConsoleMemory();
  takeConsoleTerminal();
  calls = [];
  panes.length = 0;
  await emptyConsoleWorld();
  branchRow = consoleBranchRow();
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "term-9" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  manager.attachTerminal.mockReset().mockResolvedValue({ snapshot: "", cursor: 0 });
  manager.detach.mockReset();
  notifyError.mockClear();
  bridge.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "branch.get") return branchRow;
    return {};
  });
});

afterEach(() => {
  if (panel) panel.dispose();
  panel = null;
});

// The console stands in one checkout on one machine, and the link that opened
// the surface said which. It asks that machine what is under the branch — not
// whichever machine creation happens to go to.
describe("the machine it was mounted for", () => {
  it("stands in the routed device's checkout, not in this one's", async () => {
    await seedConsoleWorld({ terminals: ["term-4"] });
    const theirs = consoleBranchRow({ deviceId: "dev-2", projectKey: "dev-2:p1", run_id: "run-9", worktree_id: "wt-9" });
    await seedConsoleWorld({ deviceId: "dev-2", row: theirs, terminals: ["term-1"] });

    await mount(branchAddress({ deviceId: "dev-2" }));
    await open();

    expect(tabs()).toEqual(["Terminal 1"]);
    expect(manager.attachTerminal).toHaveBeenCalledWith("term-1", { run_id: "run-9" }, expect.anything());
    // Nothing was asked of any machine to draw that.
    expect(calls).toEqual([]);
  });
});

describe("the shut console", () => {
  it("is a bar with the checkout's terminals beside it, and creates none of them", async () => {
    await mountOver(["term-1"]);
    expect(size()).toBe("collapsed");
    expect(bar().getAttribute("aria-expanded")).toBe("false");
    expect(bar().textContent).toContain("Console");
    expect(tabs()).toEqual(["Terminal 1"]);
    expect(region().querySelector(".console-new")).toBeTruthy();
    expect(manager.createTerminal).not.toHaveBeenCalled();
    expect(manager.attachTerminal).not.toHaveBeenCalled();
  });
});

describe("opening it", () => {
  it("puts it at half and shows the terminals of the branch's own worktree", async () => {
    await mountOver(["term-1", "term-2"]);
    await open();
    expect(size()).toBe("half");
    expect(bar().getAttribute("aria-expanded")).toBe("true");
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    // The first terminal is what it lands on, in the checkout the row names.
    expect(manager.attachTerminal).toHaveBeenCalledWith("term-1", { run_id: "run-3" }, expect.anything());
  });

  // The row is read off the disk, never off the wire: a console that asked the
  // daemon which directory it stands in would have it serialize the branch's
  // whole conversation to answer.
  it("asks the machine nothing to say where it is standing", async () => {
    await mountOver(["term-1"]);
    await open();
    expect(calls).toEqual([]);
  });

  it("opens a checkout Build never cut by the worktree itself", async () => {
    const loose = consoleBranchRow({ branch: "loose", run_id: null, worktree_id: "wt-9" });
    await seedConsoleWorld({ row: loose, terminals: ["term-1"] });
    await mount(branchAddress({ branch: "loose" }));
    await open();
    expect(manager.attachTerminal).toHaveBeenCalledWith(
      "term-1",
      { project_id: "p1", worktree_id: "wt-9" },
      expect.anything(),
    );
  });

  it("opens an issue's console on the primary checkout, without a row to read", async () => {
    await emptyConsoleWorld();
    await seedConsoleTerminals(["term-1"], { row: { run_id: "i-1" } });
    await mount({ kind: "issue", deviceId: "dev-1", projectId: "p1", issueId: "i-1", call: (...args) => bridge.call(...args), cacheScope: consoleCacheScope() });
    await open();
    expect(manager.attachTerminal).toHaveBeenCalledWith("term-1", { project_id: "p1" }, expect.anything());
    expect(calls).toEqual([]);
  });

  it("never spawns a shell just because it was opened", async () => {
    await mountOver(["term-1"]);
    await open();
    expect(manager.createTerminal).not.toHaveBeenCalled();
  });

  it("says so when no row on this device names a directory to stand in", async () => {
    await seedConsoleWorld({ row: null });
    await mount();
    await open();
    expect(region().textContent).toContain("no checkout here");
  });

  it("stands itself up when the row it is waiting for lands", async () => {
    await seedConsoleWorld({ row: null });
    await mount();
    expect(tabs()).toEqual([]);
    await seedConsoleWorld({ terminals: ["term-1"] });
    await flush();
    expect(tabs()).toEqual(["Terminal 1"]);
  });
});

describe("the sizes", () => {
  it("grows to the overlay and back to half", async () => {
    await mountOver(["term-1"]);
    await open();
    region().querySelector(".console-grow").click();
    await flush();
    expect(size()).toBe("full");
    // The screen is not torn down to change how much room it has.
    expect(panes.length).toBe(1);
    region().querySelector(".console-grow").click();
    await flush();
    expect(size()).toBe("half");
    expect(panes.length).toBe(1);
  });

  it("shuts from the bar, dropping the screen and leaving the PTY running", async () => {
    await mountOver(["term-1"]);
    await open();
    await open(); // the same control shuts it
    expect(size()).toBe("collapsed");
    expect(panes[0].disposed).toBe(true);
    expect(manager.detach).toHaveBeenCalledWith("term-1");
    expect(manager.closeTerminal).not.toHaveBeenCalled();
  });

  it("remembers the size per work item, and reopens there", async () => {
    await mountOver(["term-1"]);
    await open();
    panel.dispose();
    await mount();
    expect(size()).toBe("half");
    // Another work item's console is its own, and starts shut.
    panel.dispose();
    await mount({ kind: "issue", deviceId: "dev-1", projectId: "p1", issueId: "i-1", call: (...args) => bridge.call(...args), cacheScope: consoleCacheScope() });
    expect(size()).toBe("collapsed");
  });

  it("repaints an open console when its cached fold record changes", async () => {
    await writeCached(sizeAddress(), { size: "half", reopenSize: "half" });
    await seedConsoleWorld({ terminals: ["term-1"] });
    panel = mountConsole(region(), branchAddress());
    await vi.waitFor(() => expect(size()).toBe("half"));

    await writeCached(sizeAddress(), { size: "full", reopenSize: "full" });
    await vi.waitFor(() => expect(size()).toBe("full"));
    expect(region().querySelector(".console-grow").getAttribute("aria-label")).toBe("Half the view");
  });
});

describe("the terminals", () => {
  it("opens one from the +, and lands on it", async () => {
    await mountOver(["term-1"]);
    await open();
    region().querySelector(".console-new").click();
    await flush();
    expect(manager.createTerminal).toHaveBeenCalledWith({ run_id: "run-3" }, 80, 24);
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    expect(region().querySelector(".console-tab.active .console-tab-name").textContent).toBe("Terminal 2");
  });

  it("opens the first one from the + in the shut head", async () => {
    await mountOver([]);
    region().querySelector(".console-new").click();
    await flush();
    expect(manager.createTerminal).toHaveBeenCalled();
    expect(tabs()).toEqual(["Terminal 1"]);
    expect(size()).toBe("half");
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-9");
  });

  it("switches between them", async () => {
    await mountOver(["term-1", "term-2"]);
    await open();
    [...region().querySelectorAll(".console-tab-name")][1].click();
    await flush();
    expect(manager.attachTerminal.mock.calls.map((call) => call[0])).toEqual(["term-1", "term-2"]);
    expect(manager.detach).toHaveBeenCalledWith("term-1");
  });

  it("closes one from its ×, and falls back to what is left", async () => {
    await mountOver(["term-1", "term-2"]);
    await open();
    region().querySelector('[data-close="term-1"]').click();
    await flush();
    expect(manager.closeTerminal).toHaveBeenCalledWith("term-1");
    expect(tabs()).toEqual(["Terminal 1"]); // the ordinals close up
    expect(region().querySelector(".console-tab.active")).toBeTruthy();
  });

  it("drops a terminal the machine no longer knows instead of showing a dead pane", async () => {
    manager.attachTerminal.mockRejectedValue(new Error("unknown term_id"));
    await mountOver(["term-1"]);
    await open();
    expect(tabs()).toEqual([]);
    expect(size()).toBe("collapsed");
    expect(region().querySelector(".console-pane")).toBeNull();
  });
});

describe("a console with no terminals", () => {
  const storedSize = async () => (await readCached(sizeAddress()))?.value?.size ?? null;

  it("opens on the tab that was pressed", async () => {
    await mountOver(["term-1", "term-2"]);
    expect(size()).toBe("collapsed");
    [...region().querySelectorAll(".console-tab-name")][1].click();
    await flush();
    expect(size()).toBe("half");
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-2");
  });

  it("stays shut when the label is pressed with nothing to show", async () => {
    await mountOver([]);
    await open();
    expect(size()).toBe("collapsed");
    expect(region().querySelector(".console-body").textContent).toBe("");
    expect(await storedSize()).toBeNull();
  });

  it("is the same control under the backtick, and just as inert", async () => {
    await mountOver([]);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "`", bubbles: true, cancelable: true }));
    await flush();
    expect(size()).toBe("collapsed");
    expect(await storedSize()).toBeNull();
  });

  it("shuts when the last terminal is closed, and reopens at the size it was left", async () => {
    await mountOver(["term-1"]);
    await open();
    region().querySelector(".console-grow").click();
    await flush();
    expect(size()).toBe("full");
    region().querySelector('[data-close="term-1"]').click();
    await flush();
    expect(size()).toBe("collapsed");
    expect(await storedSize()).toBe("full");
    region().querySelector(".console-new").click();
    await flush();
    expect(size()).toBe("full");
  });

  it("renders shut when the size it remembers is open and nothing is running", async () => {
    await writeCached(sizeAddress(), { size: "half", reopenSize: "half" });
    await mountOver([]);
    expect(size()).toBe("collapsed");
    expect(region().querySelector(".console-body").textContent).toBe("");
  });

  it("says why a terminal could not be opened, and opens no empty panel for it", async () => {
    manager.createTerminal.mockRejectedValue(new Error("no such directory"));
    await mountOver([]);
    region().querySelector(".console-new").click();
    await flush();
    expect(notifyError).toHaveBeenCalledWith("Could not open a terminal", "no such directory");
    expect(size()).toBe("collapsed");
  });

  it("stays shut until the checkout it remembers as open has something in it", async () => {
    await writeCached(sizeAddress(), { size: "half", reopenSize: "half" });
    await seedConsoleWorld({ terminals: [] });
    panel = mountConsole(region(), branchAddress());
    await flush();

    expect(size()).toBe("collapsed");
    expect(region().querySelector(".console-body").textContent).toBe("");

    await seedConsoleTerminals(["term-1"]);
    await flush();
    expect(size()).toBe("half");
  });

  it("reopens at the size it was left at, not at half, once it has been shut", async () => {
    await mountOver(["term-1"]);
    await open();
    region().querySelector(".console-grow").click();
    await flush();
    expect(size()).toBe("full");

    await open();
    expect(size()).toBe("collapsed");
    panel.dispose();

    await mount();
    await open();
    expect(size()).toBe("full");
  });
});

describe("the backtick", () => {
  const press = (target) => {
    const event = new KeyboardEvent("keydown", { key: "`", bubbles: true, cancelable: true });
    (target || document.body).dispatchEvent(event);
    return event;
  };

  it("opens and shuts the console from anywhere else on the page", async () => {
    await mountOver(["term-1"]);
    const opened = press();
    await flush();
    expect(size()).toBe("half");
    expect(opened.defaultPrevented).toBe(true);
    press();
    await flush();
    expect(size()).toBe("collapsed");
  });

  it("is a backtick in a field, a composer and a terminal screen", async () => {
    await mountOver([]);
    const field = document.createElement("input");
    document.body.appendChild(field);
    press(field);
    await flush();
    expect(size()).toBe("collapsed");

    const composer = document.createElement("div");
    composer.className = "thread-composer";
    const child = document.createElement("div");
    composer.appendChild(child);
    document.body.appendChild(composer);
    press(child);
    await flush();
    expect(size()).toBe("collapsed");
  });

  it("stops listening once the surface is gone", async () => {
    await mountOver([]);
    panel.dispose();
    press();
    await flush();
    expect(region().innerHTML).toBe("");
    panel = null;
  });
});

describe("a pre-redesign term-<n> URL", () => {
  it("opens the branch's console on the terminal it named", async () => {
    await seedConsoleWorld({ terminals: ["term-1", "term-2"] });
    markConsoleTerminal("term-2");
    await mount();
    await flush();
    expect(size()).toBe("half");
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-2");
    expect(region().querySelector(".console-tab.active .console-tab-name").textContent).toBe("Terminal 2");
  });

  it("is spent once, so the next surface opens on what it remembers", async () => {
    await seedConsoleWorld({ terminals: ["term-1", "term-2"] });
    markConsoleTerminal("term-2");
    await mount();
    panel.dispose();
    await mount(branchAddress({ branch: "other" }));
    expect(size()).toBe("collapsed");
  });
});

describe("disposal", () => {
  it("leaves the region empty and the server PTY running", async () => {
    await mountOver(["term-1"]);
    await open();
    panel.dispose();
    expect(region().innerHTML).toBe("");
    expect(region().dataset.size).toBeUndefined();
    expect(manager.detach).toHaveBeenCalledWith("term-1");
    expect(manager.closeTerminal).not.toHaveBeenCalled();
    panel = null;
  });
});
