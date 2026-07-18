import { describe, it, expect } from "vitest";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";

describe("routeFromHash", () => {
  it("maps hashes to routes", () => {
    expect(routeFromHash("#/notifications")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/settings")).toEqual({ name: "settings" });
    // The run route keeps the #/task grammar (runs are "Tasks" in the UI), keyed by run_id.
    expect(routeFromHash("#/task/run-123/changes")).toEqual({ name: "task", id: "run-123", tab: "changes" });
    expect(routeFromHash("#/task/run-123")).toEqual({ name: "task", id: "run-123", tab: "changes" });
    // The plan route is project-scoped, keyed by plan_id.
    expect(routeFromHash("#/plan/plan-123/review")).toEqual({ name: "plan", id: "plan-123", tab: "review" });
    expect(routeFromHash("#/plan/plan-123")).toEqual({ name: "plan", id: "plan-123", tab: "review" });
    expect(routeFromHash("#/worktree/proj-1/wt-abc")).toEqual({ name: "worktree", projectId: "proj-1", worktreeId: "wt-abc", tab: "changes" });
    expect(routeFromHash("#/project/proj-1")).toEqual({ name: "project", projectId: "proj-1" });
  });

  it("parses the run (task) tab vocabulary and falls back to changes on unknown", () => {
    expect(routeFromHash("#/task/r/changes").tab).toBe("changes");
    expect(routeFromHash("#/task/r/stages").tab).toBe("stages");
    expect(routeFromHash("#/task/r/files").tab).toBe("files");
    expect(routeFromHash("#/task/r/agent").tab).toBe("agent");
    expect(routeFromHash("#/task/r/term-3").tab).toBe("term-3");
    expect(routeFromHash("#/task/r/diff").tab).toBe("changes"); // legacy Diff links land on Changes
    expect(routeFromHash("#/task/r/plan").tab).toBe("changes"); // the plan surface left the run route
    expect(routeFromHash("#/task/r/bogus").tab).toBe("changes");
  });

  it("parses the plan tab vocabulary and falls back to review on unknown", () => {
    expect(routeFromHash("#/plan/p/review").tab).toBe("review");
    expect(routeFromHash("#/plan/p/agent").tab).toBe("agent");
    expect(routeFromHash("#/plan/p/term-1").tab).toBe("review"); // plans are not a terminal scope
    expect(routeFromHash("#/plan/p/bogus").tab).toBe("review");
  });

  it("deep-links a plan stage via a 4th segment (the run's Stages tab links here)", () => {
    expect(routeFromHash("#/plan/plan-1/review/second-half")).toEqual({
      name: "plan",
      id: "plan-1",
      tab: "review",
      stage: "second-half",
    });
    // No 4th segment → no stage key at all (so an equality check stays clean).
    expect(routeFromHash("#/plan/plan-1/review").stage).toBeUndefined();
    expect(routeFromHash("#/plan/a%20b/review/s%20x").stage).toBe("s x");
  });

  it("parses the worktree tab vocabulary and defaults to changes", () => {
    expect(routeFromHash("#/worktree/p/w/changes").tab).toBe("changes");
    expect(routeFromHash("#/worktree/p/w/diff").tab).toBe("changes"); // legacy Diff links land on Changes
    expect(routeFromHash("#/worktree/p/w/files").tab).toBe("files");
    expect(routeFromHash("#/worktree/p/w/term-2").tab).toBe("term-2");
    expect(routeFromHash("#/worktree/p/w/agent").tab).toBe("changes"); // no agent on worktrees
    expect(routeFromHash("#/worktree/p/w").tab).toBe("changes");
  });

  it("parses the main surface and its tab vocabulary", () => {
    expect(routeFromHash("#/main/proj-1")).toEqual({ name: "main", projectId: "proj-1", tab: "changes" });
    expect(routeFromHash("#/main/proj-1/changes").tab).toBe("changes");
    expect(routeFromHash("#/main/proj-1/files").tab).toBe("files");
    expect(routeFromHash("#/main/proj-1/term-9").tab).toBe("term-9");
    expect(routeFromHash("#/main/proj-1/bogus").tab).toBe("changes");
    expect(routeFromHash("#/main/a%20b/files")).toEqual({ name: "main", projectId: "a b", tab: "files" });
  });

  it("defaults unknown or empty hashes to notifications (the landing surface)", () => {
    expect(routeFromHash("")).toEqual({ name: "notifications" });
    expect(routeFromHash("#")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/nope")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/task")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/plan")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/worktree")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/worktree/proj-1")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/main")).toEqual({ name: "notifications" });
    expect(routeFromHash("#/project")).toEqual({ name: "notifications" });
  });

  it("redirects stale #/board hashes to notifications (the board is gone)", () => {
    expect(routeFromHash("#/board")).toEqual({ name: "notifications" });
  });

  it("decodes worktree route segments", () => {
    expect(routeFromHash("#/worktree/a%20b/wt-1")).toEqual({ name: "worktree", projectId: "a b", worktreeId: "wt-1", tab: "changes" });
  });

  it("decodes run and plan ids", () => {
    expect(routeFromHash("#/task/a%20b/changes").id).toBe("a b");
    expect(routeFromHash("#/plan/a%20b/review").id).toBe("a b");
  });
});

describe("hashFromRoute", () => {
  it("is the inverse of routeFromHash", () => {
    for (const route of [
      { name: "notifications" },
      { name: "settings" },
      { name: "task", id: "run-9", tab: "changes" },
      { name: "task", id: "a b", tab: "stages" },
      { name: "task", id: "run-9", tab: "files" },
      { name: "task", id: "run-9", tab: "agent" },
      { name: "task", id: "run-9", tab: "term-4" },
      { name: "plan", id: "plan-9", tab: "review" },
      { name: "plan", id: "a b", tab: "agent" },
      { name: "worktree", projectId: "proj-1", worktreeId: "wt-abc", tab: "changes" },
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

  it("round-trips a plan stage deep-link", () => {
    const route = { name: "plan", id: "plan-1", tab: "review", stage: "second-half" };
    expect(hashFromRoute(route)).toBe("#/plan/plan-1/review/second-half");
    expect(routeFromHash(hashFromRoute(route))).toEqual(route);
  });

  it("maps unknown route names (including the retired board) to #/notifications", () => {
    expect(hashFromRoute({ name: "board" })).toBe("#/notifications");
    expect(hashFromRoute({ name: "whatever" })).toBe("#/notifications");
  });

  it("supplies the default tab when a route omits it", () => {
    expect(hashFromRoute({ name: "task", id: "r" })).toBe("#/task/r/changes");
    expect(hashFromRoute({ name: "plan", id: "p" })).toBe("#/plan/p/review");
    expect(hashFromRoute({ name: "worktree", projectId: "p", worktreeId: "w" })).toBe("#/worktree/p/w/changes");
    expect(hashFromRoute({ name: "main", projectId: "p" })).toBe("#/main/p/changes");
  });
});
