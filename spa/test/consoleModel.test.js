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
  readConsoleSize,
  takeConsoleTerminal,
  toggledConsoleSize,
  writeConsoleSize,
} from "../src/core/consoleModel.js";

const fakeStorage = () => {
  const values = new Map();
  return {
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
    values,
  };
};

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

  it("grows a bar or a half into the overlay, and the overlay back to half", () => {
    expect(grownConsoleSize("collapsed")).toBe("full");
    expect(grownConsoleSize("half")).toBe("full");
    expect(grownConsoleSize("full")).toBe("half");
  });
});

describe("the size a work item is remembered at", () => {
  it("keys the memory by the branch or the issue, so each is its own console", () => {
    expect(consoleKey({ kind: "branch", projectId: "p1", branch: "build/login" })).toBe("branch:p1:build/login");
    expect(consoleKey({ kind: "issue", projectId: "p1", issueId: "i-7" })).toBe("issue:i-7");
  });

  it("round-trips through the device's storage, shut until something is chosen", () => {
    const storage = fakeStorage();
    const key = consoleKey({ kind: "branch", projectId: "p1", branch: "main" });
    expect(readConsoleSize(key, storage)).toBe("collapsed");
    writeConsoleSize(key, "half", storage);
    expect(readConsoleSize(key, storage)).toBe("half");
    // One entity's choice says nothing about another's.
    expect(readConsoleSize(consoleKey({ kind: "issue", projectId: "p1", issueId: "i-2" }), storage)).toBe("collapsed");
  });

  it("survives a storage that refuses to answer", () => {
    const broken = {
      getItem() {
        throw new Error("private mode");
      },
      setItem() {
        throw new Error("private mode");
      },
    };
    expect(readConsoleSize("k", broken)).toBe("collapsed");
    expect(() => writeConsoleSize("k", "half", broken)).not.toThrow();
  });
});

describe("whose terminals the console holds", () => {
  it("scopes a branch to the run's worktree when Build cut one", () => {
    const context = { kind: "branch", projectId: "p1", branch: "build/login" };
    const row = { project_id: "p1", run_id: "run-3", worktree_id: "wt-3", primary: false };
    expect(consoleScope(context, row)).toEqual({ run_id: "run-3" });
  });

  it("scopes a checkout Build never cut to the worktree itself", () => {
    const context = { kind: "branch", projectId: "p1", branch: "loose" };
    const row = { project_id: "p1", run_id: null, worktree_id: "wt-9", primary: false };
    expect(consoleScope(context, row)).toEqual({ project_id: "p1", worktree_id: "wt-9" });
  });

  it("scopes main to the primary checkout — the repository itself", () => {
    const context = { kind: "branch", projectId: "p1", branch: "main" };
    const row = { project_id: "p1", run_id: null, worktree_id: null, primary: true };
    expect(consoleScope(context, row)).toEqual({ project_id: "p1" });
  });

  it("scopes an issue to the primary checkout, where its agent runs", () => {
    expect(consoleScope({ kind: "issue", projectId: "p1", issueId: "i-1" }, null)).toEqual({ project_id: "p1" });
  });

  it("names no directory when the branch answered with nothing to stand in", () => {
    const context = { kind: "branch", projectId: "p1", branch: "gone" };
    expect(consoleScope(context, null)).toBeNull();
    expect(consoleScope(context, { project_id: "p1", run_id: null, worktree_id: null, primary: false })).toBeNull();
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
