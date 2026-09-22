import { describe, expect, it } from "vitest";
import { directoryCacheId, syncWalksCheckout } from "../src/core/directoryScope.js";

// A workspace's git directory is what its conversation's run stands on for git:
// the bridge answers run-scoped reads from it and pushes its facts under the
// run. So a pane over that directory files its records under the workspace's
// entity — where the sync layer writes and the pushes land — and lets the sync
// walk it rather than reading for itself.
describe("a workspace directory that is the workspace's own git", () => {
  const scope = { workspace_id: "ws-1", source_id: "source-1", entity_id: "run-7" };

  it("files its records under the workspace's entity", () => {
    expect(directoryCacheId(scope)).toBe("run-7");
  });

  it("is walked by the sync layer", () => {
    expect(syncWalksCheckout(scope)).toBe(true);
  });
});

describe("a workspace directory with no entity of its own", () => {
  const scope = { workspace_id: "ws-1", source_id: "source-2" };

  it("keeps a directory-local record namespace", () => {
    expect(directoryCacheId(scope)).toBe('workspace:["ws-1","source-2"]');
  });

  it("reads for itself", () => {
    expect(syncWalksCheckout(scope)).toBe(false);
  });
});

describe("a run or a bare checkout", () => {
  it("is its own entity and is walked", () => {
    expect(directoryCacheId({ run_id: "run-1" })).toBe("run-1");
    expect(directoryCacheId({ project_id: "p", worktree_id: "wt-1" })).toBe("wt-1");
    expect(syncWalksCheckout({ run_id: "run-1" })).toBe(true);
    expect(syncWalksCheckout(null)).toBe(false);
  });

  it("gives a project's own checkout a synthetic cache entity", () => {
    expect(directoryCacheId({ project_id: "project-1" })).toBe('project:["project-1"]');
    expect(syncWalksCheckout({ project_id: "project-1" })).toBe(false);
  });
});
