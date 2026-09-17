// The console's pure model: how big it is, whose terminals it holds, what a
// remembered size means on the next visit, and when a keystroke belongs to it.

import { describe, it, expect, beforeEach } from "vitest";
import {
  CONSOLE_SIZES,
  consoleKey,
  consoleScope,
  consoleSize,
  consoleTakesKey,
  grownConsoleSize,
  markConsoleTerminal,
  readConsoleReopenSize,
  readConsoleSize,
  takeConsoleTerminal,
  toggledConsoleSize,
  writeConsoleReopenSize,
  writeConsoleSize,
} from "../src/core/consoleModel.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

describe("the three sizes", () => {
  it("is a bar, a half and a full overlay — and nothing else", () => {
    expect(CONSOLE_SIZES).toEqual(["collapsed", "half", "full"]);
  });

  it("reads an unknown or missing size as the shut bar", () => {
    expect(consoleSize(null)).toBe("collapsed");
    expect(consoleSize("")).toBe("collapsed");
    expect(consoleSize("enormous")).toBe("collapsed");
    expect(consoleSize("full")).toBe("full");
  });

  it("toggles between shut and half, and shuts a full one", () => {
    expect(toggledConsoleSize("collapsed")).toBe("half");
    expect(toggledConsoleSize("half")).toBe("collapsed");
    expect(toggledConsoleSize("full")).toBe("collapsed");
  });

  it("opens a shut console at the size it was last open at", () => {
    expect(toggledConsoleSize("collapsed", "full")).toBe("full");
    expect(toggledConsoleSize("collapsed", "half")).toBe("half");
    expect(toggledConsoleSize("full", "full")).toBe("collapsed");
  });

  it("takes neither shut nor a size that is not one as a size to open at", () => {
    expect(toggledConsoleSize("collapsed", "collapsed")).toBe("half");
    expect(toggledConsoleSize("collapsed", "sideways")).toBe("half");
  });

  it("grows a bar or a half into the overlay, and the overlay back to half", () => {
    expect(grownConsoleSize("collapsed")).toBe("full");
    expect(grownConsoleSize("half")).toBe("full");
    expect(grownConsoleSize("full")).toBe("half");
  });
});

describe("the size a console reopens at", () => {
  const KEY = "branch:p1:build/login";

  it("remembers the open size it was last drawn at, and never that it was shut", () => {
    const storage = memoryStorage();
    writeConsoleReopenSize(KEY, "full", storage);
    expect(readConsoleReopenSize(KEY, storage)).toBe("full");

    writeConsoleReopenSize(KEY, "collapsed", storage);

    expect(readConsoleReopenSize(KEY, storage)).toBe("full");
  });

  it("opens at half where nothing was remembered, or nonsense was", () => {
    const storage = memoryStorage();
    expect(readConsoleReopenSize(KEY, storage)).toBe("half");
    storage.setItem("build.console.reopen." + KEY, "sideways");
    expect(readConsoleReopenSize(KEY, storage)).toBe("half");
    expect(readConsoleReopenSize(KEY, refusingStorage())).toBe("half");
  });
});

describe("the size a work item is remembered at", () => {
  it("keys the memory by the branch or the issue, so each is its own console", () => {
    expect(consoleKey({ kind: "branch", projectId: "p1", branch: "build/login" })).toBe("branch:p1:build/login");
    expect(consoleKey({ kind: "issue", projectId: "p1", issueId: "i-7" })).toBe("issue:i-7");
    // A workspace lives on a machine, and two machines can hand out the same
    // workspace id, so the key carries the device the way every other
    // workspace key does (core/deviceKey.js mints it).
    expect(consoleKey({ kind: "workspace", deviceId: "dev-1", workspaceId: "ws-7", sourceId: "src-2" })).toBe(
      "workspace:dev-1/ws-7",
    );
    expect(consoleKey({ kind: "workspace", deviceId: "dev-2", workspaceId: "ws-7" })).toBe("workspace:dev-2/ws-7");
  });

  it("round-trips through the device's storage, shut until something is chosen", () => {
    const storage = memoryStorage();
    const key = consoleKey({ kind: "branch", projectId: "p1", branch: "main" });
    expect(readConsoleSize(key, storage)).toBe("collapsed");
    writeConsoleSize(key, "half", storage);
    expect(readConsoleSize(key, storage)).toBe("half");
    // One entity's choice says nothing about another's.
    expect(readConsoleSize(consoleKey({ kind: "issue", projectId: "p1", issueId: "i-2" }), storage)).toBe("collapsed");
  });

  it("survives a storage that refuses to answer", () => {
    const broken = refusingStorage();
    expect(readConsoleSize("k", broken)).toBe("collapsed");
    expect(() => writeConsoleSize("k", "half", broken)).not.toThrow();
  });
});

describe("whose terminals the console holds", () => {
  it("scopes terminals to the workspace and ignores the selected source", () => {
    expect(consoleScope({ kind: "workspace", workspaceId: "ws-7", sourceId: "src-2" }, null)).toEqual({ workspace_id: "ws-7" });
  });

  it("scopes a branch to the run's worktree when Build cut one", () => {
    const context = { kind: "branch", projectId: "p1", branch: "build/login" };
    const row = { project_id: "p1", run_id: "run-3", worktree_id: "wt-3", primary: false };
    expect(consoleScope(context, row)).toEqual({ run_id: "run-3" });
  });

  it("scopes a checkout Build never cut to the worktree itself", () => {
    const context = { kind: "branch", projectId: "p1", branch: "loose" };
    const row = { project_id: "p1", run_id: null, worktree_id: "wt-9" };
    expect(consoleScope(context, row)).toEqual({ project_id: "p1", worktree_id: "wt-9" });
  });

  // A row that names neither a run nor a worktree is the project's own
  // directory — the repository this branch is checked out in, or a plain folder
  // with no git in it. The project alone names it.
  it("scopes a row with no checkout under it to the project's own directory", () => {
    const context = { kind: "branch", projectId: "p1", branch: "main" };
    expect(consoleScope(context, { project_id: "p1", run_id: null, worktree_id: null, is_git: false })).toEqual({ project_id: "p1" });
    expect(consoleScope(context, { project_id: "p1", run_id: null, worktree_id: null })).toEqual({ project_id: "p1" });
    // The project comes off the route when the row does not carry one.
    expect(consoleScope(context, { run_id: null, worktree_id: null })).toEqual({ project_id: "p1" });
  });

  it("scopes an issue to the project's own checkout, where its agent runs", () => {
    expect(consoleScope({ kind: "issue", projectId: "p1", issueId: "i-1" }, null)).toEqual({ project_id: "p1" });
  });

  it("names no directory when the branch answered with nothing at all", () => {
    const context = { kind: "branch", projectId: "p1", branch: "gone" };
    expect(consoleScope(context, null)).toBeNull();
    expect(consoleScope({ kind: "branch", projectId: null, branch: "gone" }, { run_id: null })).toBeNull();
    expect(consoleScope({ kind: "issue", projectId: null, issueId: "i-1" }, null)).toBeNull();
  });
});

describe("the backtick belongs to the console only when nothing else is listening", () => {
  const element = (tag, over = {}) => ({
    tagName: tag,
    isContentEditable: false,
    closest: () => null,
    ...over,
  });

  it("takes the key from the page and from a plain element", () => {
    expect(consoleTakesKey(null)).toBe(true);
    expect(consoleTakesKey(element("DIV"))).toBe(true);
    expect(consoleTakesKey(element("BUTTON"))).toBe(true);
  });

  it("leaves it alone in anything the user is typing into", () => {
    expect(consoleTakesKey(element("INPUT"))).toBe(false);
    expect(consoleTakesKey(element("TEXTAREA"))).toBe(false);
    expect(consoleTakesKey(element("SELECT"))).toBe(false);
    expect(consoleTakesKey(element("DIV", { isContentEditable: true }))).toBe(false);
  });

  it("leaves it alone inside a terminal screen or a conversation composer", () => {
    const inside = (selector) => element("DIV", { closest: (query) => (query.includes(selector) ? {} : null) });
    expect(consoleTakesKey(inside("termpane"))).toBe(false);
    expect(consoleTakesKey(inside("thread-composer"))).toBe(false);
  });
});

describe("a legacy term-<n> URL", () => {
  beforeEach(() => {
    takeConsoleTerminal();
  });

  it("hands the terminal to the next console that opens, once", () => {
    markConsoleTerminal("term-3");
    expect(takeConsoleTerminal()).toBe("term-3");
    expect(takeConsoleTerminal()).toBeNull();
  });

  it("carries nothing that does not name a terminal", () => {
    markConsoleTerminal("changes");
    markConsoleTerminal("");
    markConsoleTerminal(null);
    expect(takeConsoleTerminal()).toBeNull();
  });
});
