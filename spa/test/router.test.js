import { describe, it, expect } from "vitest";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";

describe("routeFromHash", () => {
  it("maps hashes to routes", () => {
    expect(routeFromHash("#/board")).toEqual({ name: "board" });
    expect(routeFromHash("#/notifications")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/settings")).toEqual({ name: "settings" });
    expect(routeFromHash("#/task/t-123/diff")).toEqual({ name: "task", id: "t-123", tab: "diff" });
    expect(routeFromHash("#/task/t-123")).toEqual({ name: "task", id: "t-123", tab: "plan" });
  });

  it("defaults unknown or empty hashes to the board", () => {
    expect(routeFromHash("")).toEqual({ name: "board" });
    expect(routeFromHash("#")).toEqual({ name: "board" });
    expect(routeFromHash("#/nope")).toEqual({ name: "board" });
    expect(routeFromHash("#/task")).toEqual({ name: "board" });
  });

  it("decodes task ids", () => {
    expect(routeFromHash("#/task/a%20b/plan").id).toBe("a b");
  });
});

describe("hashFromRoute", () => {
  it("is the inverse of routeFromHash", () => {
    for (const route of [
      { name: "board" },
      { name: "notifications" },
      { name: "settings" },
      { name: "task", id: "t-9", tab: "diff" },
      { name: "task", id: "a b", tab: "plan" },
    ]) {
      expect(routeFromHash(hashFromRoute(route))).toEqual(route);
    }
  });
});
