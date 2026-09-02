// Which face the inbox rail remembers — the one list, or the projects — and
// which project blocks the user folded shut.

import { describe, it, expect } from "vitest";
import {
  RAIL_VIEW_KEY,
  RAIL_VIEWS,
  FOLDED_PROJECTS_KEY,
  loadRailView,
  persistRailView,
  loadFoldedProjects,
  persistFoldedProjects,
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

describe("which projects the rail remembers folded", () => {
  it("starts with every project open", () => {
    expect(loadFoldedProjects(memoryStorage())).toEqual(new Set());
  });

  it("remembers the folded projects by id, and forgets one that is opened again", () => {
    const store = memoryStorage();
    persistFoldedProjects(new Set(["p2", "p3"]), store);
    expect(loadFoldedProjects(store)).toEqual(new Set(["p2", "p3"]));
    persistFoldedProjects(new Set(["p3"]), store);
    expect(loadFoldedProjects(store)).toEqual(new Set(["p3"]));
  });

  it("treats a stored value that is not a list of ids as nothing folded", () => {
    expect(loadFoldedProjects(memoryStorage({ [FOLDED_PROJECTS_KEY]: "p2" }))).toEqual(new Set());
    expect(loadFoldedProjects(memoryStorage({ [FOLDED_PROJECTS_KEY]: '{"a":1}' }))).toEqual(new Set());
    expect(loadFoldedProjects(memoryStorage({ [FOLDED_PROJECTS_KEY]: "[1, null]" }))).toEqual(new Set());
  });

  it("does not throw when the storage refuses either way", () => {
    expect(loadFoldedProjects(refusingStorage())).toEqual(new Set());
    expect(() => persistFoldedProjects(new Set(["p1"]), refusingStorage())).not.toThrow();
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
