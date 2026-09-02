import { describe, it, expect } from "vitest";
import { loadRailMode, persistRailMode, RAIL_MODE_KEY, RAIL_MODES } from "../src/core/railMode.js";
import { memoryStorage, refusingStorage } from "./memoryStorage.js";

describe("which rail the sidebar remembers", () => {
  it("offers exactly the two modes", () => {
    expect(RAIL_MODES).toEqual(["projects", "all"]);
  });

  it("starts on projects when nothing has been chosen yet", () => {
    expect(loadRailMode(memoryStorage())).toBe("projects");
  });

  it("falls back to projects when the stored value means nothing", () => {
    expect(loadRailMode(memoryStorage({ [RAIL_MODE_KEY]: "everything" }))).toBe("projects");
    expect(loadRailMode(memoryStorage({ [RAIL_MODE_KEY]: "" }))).toBe("projects");
  });

  // The remembered mode is a convenience, never fatal: a browser with storage
  // switched off still gets a rail.
  it("falls back to projects when the storage refuses to answer", () => {
    expect(loadRailMode(refusingStorage())).toBe("projects");
  });

  it("remembers the flat rail once you choose it, and lets you go back", () => {
    const store = memoryStorage();
    persistRailMode("all", store);
    expect(loadRailMode(store)).toBe("all");
    persistRailMode("projects", store);
    expect(loadRailMode(store)).toBe("projects");
  });

  it("refuses to remember a mode it does not have", () => {
    const store = memoryStorage();
    persistRailMode("everything", store);
    expect(loadRailMode(store)).toBe("projects");
  });

  it("does not throw when the storage refuses to be written", () => {
    expect(() => persistRailMode("all", refusingStorage())).not.toThrow();
  });
});
