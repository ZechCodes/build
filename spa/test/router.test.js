import { describe, it, expect } from "vitest";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";

describe("routeFromHash", () => {
  it("maps hashes to routes", () => {
    expect(routeFromHash("#/board")).toEqual({ name: "board" });
    expect(routeFromHash("#/notifications")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/settings")).toEqual({ name: "settings" });
    expect(routeFromHash("#/task/t-123/changes")).toEqual({ name: "task", id: "t-123", tab: "changes" });
    expect(routeFromHash("#/task/t-123")).toEqual({ name: "task", id: "t-123", tab: "plan" });
    expect(routeFromHash("#/worktree/proj-1/wt-abc")).toEqual({ name: "worktree", projectId: "proj-1", worktreeId: "wt-abc", tab: "diff" });
    expect(routeFromHash("#/project/proj-1")).toEqual({ name: "project", projectId: "proj-1" });
  });

  it("parses the task tab vocabulary and falls back to plan on unknown", () => {
    expect(routeFromHash("#/task/t/plan").tab).toBe("plan");
    expect(routeFromHash("#/task/t/changes").tab).toBe("changes");
    expect(routeFromHash("#/task/t/diff").tab).toBe("changes"); // legacy links land on Changes
    expect(routeFromHash("#/task/t/files").tab).toBe("files");
    expect(routeFromHash("#/task/t/agent").tab).toBe("agent");
    expect(routeFromHash("#/task/t/term-3").tab).toBe("term-3");
    expect(routeFromHash("#/task/t/bogus").tab).toBe("plan");
  });

  it("parses the worktree tab vocabulary and defaults to diff", () => {
    expect(routeFromHash("#/worktree/p/w/diff").tab).toBe("diff");
    expect(routeFromHash("#/worktree/p/w/files").tab).toBe("files");
    expect(routeFromHash("#/worktree/p/w/term-2").tab).toBe("term-2");
    expect(routeFromHash("#/worktree/p/w/agent").tab).toBe("diff"); // no agent on worktrees
    expect(routeFromHash("#/worktree/p/w").tab).toBe("diff");
  });

  it("parses the main surface and its tab vocabulary", () => {
    expect(routeFromHash("#/main/proj-1")).toEqual({ name: "main", projectId: "proj-1", tab: "changes" });
    expect(routeFromHash("#/main/proj-1/changes").tab).toBe("changes");
    expect(routeFromHash("#/main/proj-1/files").tab).toBe("files");
    expect(routeFromHash("#/main/proj-1/term-9").tab).toBe("term-9");
    expect(routeFromHash("#/main/proj-1/bogus").tab).toBe("changes");
    expect(routeFromHash("#/main/a%20b/files")).toEqual({ name: "main", projectId: "a b", tab: "files" });
  });

  it("defaults unknown or empty hashes to the board", () => {
    expect(routeFromHash("")).toEqual({ name: "board" });
    expect(routeFromHash("#")).toEqual({ name: "board" });
    expect(routeFromHash("#/nope")).toEqual({ name: "board" });
    expect(routeFromHash("#/task")).toEqual({ name: "board" });
    expect(routeFromHash("#/worktree")).toEqual({ name: "board" });
    expect(routeFromHash("#/worktree/proj-1")).toEqual({ name: "board" });
    expect(routeFromHash("#/main")).toEqual({ name: "board" });
    expect(routeFromHash("#/project")).toEqual({ name: "board" });
  });

  it("decodes worktree route segments", () => {
    expect(routeFromHash("#/worktree/a%20b/wt-1")).toEqual({ name: "worktree", projectId: "a b", worktreeId: "wt-1", tab: "diff" });
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
      { name: "task", id: "t-9", tab: "changes" },
      { name: "task", id: "a b", tab: "plan" },
      { name: "task", id: "t-9", tab: "files" },
      { name: "task", id: "t-9", tab: "agent" },
      { name: "task", id: "t-9", tab: "term-4" },
      { name: "worktree", projectId: "proj-1", worktreeId: "wt-abc", tab: "diff" },
      { name: "worktree", projectId: "a b", worktreeId: "wt x", tab: "files" },
      { name: "worktree", projectId: "p", worktreeId: "w", tab: "term-1" },
      { name: "main", projectId: "proj-1", tab: "changes" },
      { name: "main", projectId: "a b", tab: "files" },
      { name: "main", projectId: "p", tab: "term-2" },
      { name: "project", projectId: "proj-1" },
      { name: "project", projectId: "a b" },
    ]) {
      expect(routeFromHash(hashFromRoute(route))).toEqual(route);
    }
  });

  it("supplies the default tab when a route omits it", () => {
    expect(hashFromRoute({ name: "task", id: "t" })).toBe("#/task/t/plan");
    expect(hashFromRoute({ name: "worktree", projectId: "p", worktreeId: "w" })).toBe("#/worktree/p/w/diff");
    expect(hashFromRoute({ name: "main", projectId: "p" })).toBe("#/main/p/changes");
  });
});
