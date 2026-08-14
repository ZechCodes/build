// @vitest-environment jsdom
// The console: the bar at the bottom of a work surface, the terminals of the
// checkout behind it, and the three sizes it opens to.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

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

const { App } = await import("../src/app.js");
const { mountConsole, resetConsoleMemory } = await import("../src/core/console.js");
const { markConsoleTerminal, takeConsoleTerminal } = await import("../src/core/consoleModel.js");

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
};

const region = () => document.getElementById("console-region");
const bar = () => region().querySelector(".console-bar");
const tabs = () => [...region().querySelectorAll(".console-tab-name")].map((cell) => cell.textContent);
const size = () => region().dataset.size;
const callsTo = (method) => calls.filter((call) => call.method === method);

let calls = [];
let branchRow = null;
let panel = null;

const mount = async (context = { kind: "branch", projectId: "p1", branch: "build/login" }) => {
  panel = mountConsole(region(), context);
  await flush();
  return panel;
};

const open = async () => {
  bar().click();
  await flush();
};

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetConsoleMemory();
  takeConsoleTerminal();
  calls = [];
  panes.length = 0;
  branchRow = { project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3", primary: false };
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "term-9" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  manager.attachTerminal.mockReset().mockResolvedValue({ snapshot: "", cursor: 0 });
  manager.detach.mockReset();
  App.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "branch.get") return branchRow;
    return {};
  });
});

afterEach(() => {
  if (panel) panel.dispose();
  panel = null;
});

describe("the shut console", () => {
  it("is a bar that says what it is, and asks the machine for nothing", async () => {
    await mount();
    expect(size()).toBe("collapsed");
    expect(bar().getAttribute("aria-expanded")).toBe("false");
    expect(bar().textContent).toContain("Console");
    expect(manager.listTerminals).not.toHaveBeenCalled();
    expect(callsTo("branch.get")).toEqual([]);
  });
});

describe("opening it", () => {
  it("puts it at half and lists the terminals of the branch's own worktree", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }, { term_id: "term-2" }]);
    await mount();
    await open();
    expect(size()).toBe("half");
    expect(bar().getAttribute("aria-expanded")).toBe("true");
    expect(manager.listTerminals).toHaveBeenCalledWith({ run_id: "run-3" });
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    // The first terminal is what it lands on.
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-1");
  });

  it("opens a checkout Build never cut by the worktree itself", async () => {
    branchRow = { project_id: "p1", branch: "loose", run_id: null, worktree_id: "wt-9", primary: false };
    await mount({ kind: "branch", projectId: "p1", branch: "loose" });
    await open();
    expect(manager.listTerminals).toHaveBeenCalledWith({ project_id: "p1", worktree_id: "wt-9" });
  });

  it("opens an issue's console on the primary checkout, without asking about a branch", async () => {
    await mount({ kind: "issue", projectId: "p1", issueId: "i-1" });
    await open();
    expect(manager.listTerminals).toHaveBeenCalledWith({ project_id: "p1" });
    expect(callsTo("branch.get")).toEqual([]);
  });

  it("never spawns a shell just because it was opened", async () => {
    await mount();
    await open();
    expect(manager.createTerminal).not.toHaveBeenCalled();
    expect(region().querySelector(".console-empty")).toBeTruthy();
  });

  it("says so when the branch names no directory to stand in", async () => {
    App.call = vi.fn(async () => {
      throw new Error("branch.get: no branch is checked out in this project");
    });
    await mount();
    await open();
    expect(manager.listTerminals).not.toHaveBeenCalled();
    expect(region().textContent).toContain("no checkout here");
  });
});

describe("the sizes", () => {
  it("grows to the overlay and back to half", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
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
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
    await open();
    await open(); // the same control shuts it
    expect(size()).toBe("collapsed");
    expect(panes[0].disposed).toBe(true);
    expect(manager.detach).toHaveBeenCalledWith("term-1");
    expect(manager.closeTerminal).not.toHaveBeenCalled();
  });

  it("remembers the size per work item, and reopens there", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
    await open();
    panel.dispose();
    await mount();
    expect(size()).toBe("half");
    // Another work item's console is its own, and starts shut.
    panel.dispose();
    await mount({ kind: "issue", projectId: "p1", issueId: "i-1" });
    expect(size()).toBe("collapsed");
  });
});

describe("the terminals", () => {
  it("opens one from the +, and lands on it", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
    await open();
    region().querySelector(".console-new").click();
    await flush();
    expect(manager.createTerminal).toHaveBeenCalledWith({ run_id: "run-3" }, 80, 24);
    expect(tabs()).toEqual(["Terminal 1", "Terminal 2"]);
    expect(region().querySelector(".console-tab.active .console-tab-name").textContent).toBe("Terminal 2");
  });

  it("opens the first one from the empty state", async () => {
    await mount();
    await open();
    region().querySelector(".console-start").click();
    await flush();
    expect(manager.createTerminal).toHaveBeenCalled();
    expect(tabs()).toEqual(["Terminal 1"]);
  });

  it("switches between them", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }, { term_id: "term-2" }]);
    await mount();
    await open();
    [...region().querySelectorAll(".console-tab-name")][1].click();
    await flush();
    expect(manager.attachTerminal.mock.calls.map((call) => call[0])).toEqual(["term-1", "term-2"]);
    expect(manager.detach).toHaveBeenCalledWith("term-1");
  });

  it("closes one from its ×, and falls back to what is left", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }, { term_id: "term-2" }]);
    await mount();
    await open();
    region().querySelector('[data-close="term-1"]').click();
    await flush();
    expect(manager.closeTerminal).toHaveBeenCalledWith("term-1");
    expect(tabs()).toEqual(["Terminal 1"]); // the ordinals close up
    expect(region().querySelector(".console-tab.active")).toBeTruthy();
  });

  it("drops a terminal the machine no longer knows instead of showing a dead pane", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    manager.attachTerminal.mockRejectedValue(new Error("unknown term_id"));
    await mount();
    await open();
    expect(tabs()).toEqual([]);
    expect(region().querySelector(".console-empty")).toBeTruthy();
  });
});

describe("the backtick", () => {
  const press = (target) => {
    const event = new KeyboardEvent("keydown", { key: "`", bubbles: true, cancelable: true });
    (target || document.body).dispatchEvent(event);
    return event;
  };

  it("opens and shuts the console from anywhere else on the page", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
    const opened = press();
    await flush();
    expect(size()).toBe("half");
    expect(opened.defaultPrevented).toBe(true);
    press();
    await flush();
    expect(size()).toBe("collapsed");
  });

  it("is a backtick in a field, a composer and a terminal screen", async () => {
    await mount();
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
    await mount();
    panel.dispose();
    press();
    await flush();
    expect(region().innerHTML).toBe("");
    panel = null;
  });
});

describe("a pre-redesign term-<n> URL", () => {
  it("opens the branch's console on the terminal it named", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }, { term_id: "term-2" }]);
    markConsoleTerminal("term-2");
    await mount();
    await flush();
    expect(size()).toBe("half");
    expect(manager.attachTerminal.mock.calls[0][0]).toBe("term-2");
    expect(region().querySelector(".console-tab.active .console-tab-name").textContent).toBe("Terminal 2");
  });

  it("is spent once, so the next surface opens on what it remembers", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }, { term_id: "term-2" }]);
    markConsoleTerminal("term-2");
    await mount();
    panel.dispose();
    await mount({ kind: "branch", projectId: "p1", branch: "other" });
    expect(size()).toBe("collapsed");
  });
});

describe("disposal", () => {
  it("leaves the region empty and the server PTY running", async () => {
    manager.listTerminals.mockResolvedValue([{ term_id: "term-1" }]);
    await mount();
    await open();
    panel.dispose();
    expect(region().innerHTML).toBe("");
    expect(region().dataset.size).toBeUndefined();
    expect(manager.detach).toHaveBeenCalledWith("term-1");
    expect(manager.closeTerminal).not.toHaveBeenCalled();
    panel = null;
  });
});
