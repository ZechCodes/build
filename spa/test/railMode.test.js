// Which face the inbox rail remembers — the one list, or the projects — and
// which project blocks the user folded shut.

import { describe, it, expect } from "vitest";
import {
  RAIL_VIEW_KEY,
  RAIL_VIEWS,
  FOLDED_PROJECTS_KEY,
  loadRailView,
  persistRailView,
  loadProjectFolds,
  persistProjectFolds,
  railViewSwitchHtml,
} from "../src/core/railMode.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

describe("which face the rail remembers", () => {
  it("offers exactly the two faces, the inbox first", () => {
    expect(RAIL_VIEWS).toEqual(["inbox", "projects"]);
  });

  it("starts on the inbox when nothing has been chosen yet", () => {
    expect(loadRailView(memoryStorage())).toBe("inbox");
  });

  it("falls back to the inbox when the stored value means nothing", () => {
    expect(loadRailView(memoryStorage({ [RAIL_VIEW_KEY]: "everything" }))).toBe("inbox");
    expect(loadRailView(memoryStorage({ [RAIL_VIEW_KEY]: "" }))).toBe("inbox");
  });

  // The remembered face is a convenience, never fatal: a browser with storage
  // switched off still gets a rail.
  it("falls back to the inbox when the storage refuses to answer", () => {
    expect(loadRailView(refusingStorage())).toBe("inbox");
  });

  it("remembers the projects face once you choose it, and lets you go back", () => {
    const store = memoryStorage();
    persistRailView("projects", store);
    expect(loadRailView(store)).toBe("projects");
    persistRailView("inbox", store);
    expect(loadRailView(store)).toBe("inbox");
  });

  it("refuses to remember a face it does not have", () => {
    const store = memoryStorage();
    persistRailView("everything", store);
    expect(loadRailView(store)).toBe("inbox");
  });

  it("does not throw when the storage refuses to be written", () => {
    expect(() => persistRailView("projects", refusingStorage())).not.toThrow();
  });
});

describe("what the rail remembers of each project's fold", () => {
  it("starts having been told nothing", () => {
    expect(loadProjectFolds(memoryStorage())).toEqual(new Map());
  });

  it("remembers folded and unfolded alike, by id", () => {
    const store = memoryStorage();
    persistProjectFolds(new Map([["p2", true], ["p3", false]]), store);
    expect(loadProjectFolds(store)).toEqual(new Map([["p2", true], ["p3", false]]));
    persistProjectFolds(new Map([["p3", false]]), store);
    expect(loadProjectFolds(store)).toEqual(new Map([["p3", false]]));
  });

  it("reads an older client's list of folded ids as those said folded", () => {
    expect(loadProjectFolds(memoryStorage({ [FOLDED_PROJECTS_KEY]: '["p2","p3"]' }))).toEqual(new Map([["p2", true], ["p3", true]]));
  });

  it("treats a stored value of the wrong shape as nothing said", () => {
    expect(loadProjectFolds(memoryStorage({ [FOLDED_PROJECTS_KEY]: "p2" }))).toEqual(new Map());
    expect(loadProjectFolds(memoryStorage({ [FOLDED_PROJECTS_KEY]: '{"a":1}' }))).toEqual(new Map());
    expect(loadProjectFolds(memoryStorage({ [FOLDED_PROJECTS_KEY]: "[1, null]" }))).toEqual(new Map());
    expect(loadProjectFolds(memoryStorage({ [FOLDED_PROJECTS_KEY]: "null" }))).toEqual(new Map());
  });

  it("does not throw when the storage refuses either way", () => {
    expect(loadProjectFolds(refusingStorage())).toEqual(new Map());
    expect(() => persistProjectFolds(new Map([["p1", true]]), refusingStorage())).not.toThrow();
  });
});

describe("the switch between the two faces", () => {
  it("is two icon buttons, the inbox then the projects, with the standing one pressed", () => {
    const html = railViewSwitchHtml("projects");
    const buttons = [...html.matchAll(/data-inbox-view="(\w+)"[^>]*aria-pressed="(\w+)"/g)].map((m) => [m[1], m[2]]);
    expect(buttons).toEqual([
      ["inbox", "false"],
      ["projects", "true"],
    ]);
    expect(html).toContain("<svg");
    expect(html).toContain('aria-label="Inbox"');
    expect(html).toContain('aria-label="Projects"');
  });
});
