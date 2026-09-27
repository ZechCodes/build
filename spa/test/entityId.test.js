// The one derivation of a row's entity id. entity.seen and entity.mute take a
// single id and the feed's rows name several, so this is the whole agreement.

import { describe, it, expect } from "vitest";
import { entityIdOf } from "../src/core/entityId.js";

describe("entityIdOf", () => {
  it("takes a branch row's run when Build cut it", () => {
    expect(entityIdOf({ kind: "branch", run_id: "run-1", worktree_id: "wt-1", task_id: "iss-1" })).toBe("run-1");
  });

  it("falls back to the worktree for a branch Build never cut", () => {
    expect(entityIdOf({ kind: "branch", run_id: null, worktree_id: "wt-9", task_id: null })).toBe("wt-9");
  });

  it("is the task for a task row, even when a finished implementation names a run", () => {
    // A task row only returns to the feed once its implementation is
    // terminal, and it still carries that run's id. The entry is the task.
    expect(entityIdOf({ kind: "task", task_id: "iss-2", run_id: "run-old", worktree_id: null })).toBe("iss-2");
  });

  it("has nothing to say about a row with no ids at all, or no row", () => {
    expect(entityIdOf({ kind: "branch", run_id: null, worktree_id: null, task_id: null })).toBeNull();
    expect(entityIdOf(null)).toBeNull();
  });
});
