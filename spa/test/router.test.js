import { describe, it, expect } from "vitest";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";

describe("routeFromHash", () => {
  it("maps hashes to routes", () => {
    expect(routeFromHash("#/board")).toEqual({ name: "board" });
    expect(routeFromHash("#/notifications")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/settings")).toEqual({ name: "settings" });
    expect(routeFromHash("#/task/t-123/diff")).toEqual({ name: "task", id: "t-123", tab: "diff" });
    expect(routeFromHash("#/task/t-123")).toEqual({ name: "task", id: "t-123", tab: "plan" });
    expect(routeFromHash("#/worktree/proj-1/wt-abc")).toEqual({ name: "worktree", projectId: "proj-1", worktreeId: "wt-abc" });
    expect(routeFromHash("#/project/proj-1")).toEqual({ name: "project", projectId: "proj-1" });
  });

  it("defaults unknown or empty hashes to the board", () => {
    expect(routeFromHash("")).toEqual({ name: "board" });
    expect(routeFromHash("#")).toEqual({ name: "board" });
    expect(routeFromHash("#/nope")).toEqual({ name: "board" });
    expect(routeFromHash("#/task")).toEqual({ name: "board" });
    expect(routeFromHash("#/worktree")).toEqual({ name: "board" });
    expect(routeFromHash("#/worktree/proj-1")).toEqual({ name: "board" });
    expect(routeFromHash("#/project")).toEqual({ name: "board" });
  });

  it("decodes worktree route segments", () => {
    expect(routeFromHash("#/worktree/a%20b/wt-1")).toEqual({ name: "worktree", projectId: "a b", worktreeId: "wt-1" });
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
      { name: "worktree", projectId: "proj-1", worktreeId: "wt-abc" },
      { name: "worktree", projectId: "a b", worktreeId: "wt x" },
      { name: "project", projectId: "proj-1" },
      { name: "project", projectId: "a b" },
    ]) {
      expect(routeFromHash(hashFromRoute(route))).toEqual(route);
    }
  });
});
