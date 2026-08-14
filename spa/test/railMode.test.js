import { describe, it, expect } from "vitest";
import { loadRailMode, persistRailMode, RAIL_MODE_KEY, RAIL_MODES } from "../src/core/railMode.js";

/** A Web-Storage-shaped object over a plain map, like readState's tests use. */
const storage = (initial = {}) => {
  const values = { ...initial };
  return {
    values,
    getItem: (key) => (key in values ? values[key] : null),
    setItem: (key, value) => {
      values[key] = String(value);
    },
  };
};

const brokenStorage = {
  getItem() {
    throw new Error("storage is disabled");
  },
  setItem() {
    throw new Error("storage is disabled");
  },
};

describe("which rail the sidebar remembers", () => {
  it("offers exactly the two modes", () => {
    expect(RAIL_MODES).toEqual(["projects", "all"]);
  });

  it("starts on projects when nothing has been chosen yet", () => {
    expect(loadRailMode(storage())).toBe("projects");
  });

  it("falls back to projects when the stored value means nothing", () => {
    expect(loadRailMode(storage({ [RAIL_MODE_KEY]: "everything" }))).toBe("projects");
    expect(loadRailMode(storage({ [RAIL_MODE_KEY]: "" }))).toBe("projects");
  });

  // The remembered mode is a convenience, never fatal: a browser with storage
  // switched off still gets a rail.
  it("falls back to projects when the storage refuses to answer", () => {
    expect(loadRailMode(brokenStorage)).toBe("projects");
  });

  it("remembers the flat rail once you choose it, and lets you go back", () => {
    const store = storage();
    persistRailMode("all", store);
    expect(loadRailMode(store)).toBe("all");
    persistRailMode("projects", store);
    expect(loadRailMode(store)).toBe("projects");
  });

  it("refuses to remember a mode it does not have", () => {
    const store = storage();
    persistRailMode("everything", store);
    expect(loadRailMode(store)).toBe("projects");
  });

  it("does not throw when the storage refuses to be written", () => {
    expect(() => persistRailMode("all", brokenStorage)).not.toThrow();
  });
});
