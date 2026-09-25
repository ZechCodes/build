// What the Workspaces tab says about a workspace's lifecycle (#135): the
// bridge's reclaim service writes a verdict onto every workspace row, and the
// row reads it as one line and, when nothing holds the workspace, a Reclaim.
import { describe, expect, it } from "vitest";
import { humanBytes, lifecycleView } from "../src/core/workspaceLifecycle.js";

const verdict = (extra = {}) => ({
  measured_at_ms: 1_789_916_700_000,
  last_activity_ms: 1_789_826_700_000,
  idle: true,
  reclaimable: false,
  holds: [],
  issues: [],
  dirty_files: 0,
  unpushed_commits: 0,
  behind_commits: 0,
  size_bytes: 17_200_000_000,
  pruned_bytes: 0,
  pruned_at_ms: null,
  noticed_at_ms: null,
  ...extra,
});

describe("a workspace's lifecycle line", () => {
  it("says nothing before the bridge has measured the workspace", () => {
    expect(lifecycleView(null)).toBeNull();
    expect(lifecycleView(undefined)).toBeNull();
  });

  it("says nothing about a workspace that is in use and still held", () => {
    expect(lifecycleView(verdict({ idle: false, holds: ["issue_open"] }))).toBeNull();
  });

  it("offers Reclaim with the size when nothing holds the workspace", () => {
    const view = lifecycleView(verdict({ reclaimable: true, pruned_bytes: 12_000_000_000 }));
    expect(view.reclaimable).toBe(true);
    expect(view.text).toBe("Reclaimable · 17.2 GB · 12.0 GB of build output dropped");
  });

  it("offers Reclaim on a finished workspace that is not idle yet", () => {
    const view = lifecycleView(verdict({ idle: false, reclaimable: true, size_bytes: null }));
    expect(view).toEqual({ reclaimable: true, text: "Reclaimable" });
  });

  it("names what holds an idle workspace, counted", () => {
    const view = lifecycleView(verdict({
      holds: ["dirty", "unpushed", "issue_open"],
      dirty_files: 3,
      unpushed_commits: 1,
    }));
    expect(view.reclaimable).toBe(false);
    expect(view.text).toBe("Idle · 3 uncommitted files, 1 unpushed commit, an issue not Done · 17.2 GB");
  });

  it("names the holds read live, and a measurement that ran out", () => {
    const view = lifecycleView(verdict({
      holds: ["agent_working", "terminal_open", "issues_unread", "unmeasured"],
      size_bytes: null,
    }));
    expect(view.text).toBe("Idle · an agent working, a terminal open, issues unread, not fully measured");
  });

  it("reads a hold this build has never heard of as the bridge's word", () => {
    expect(lifecycleView(verdict({ holds: ["frozen_moon"], size_bytes: null })).text).toBe("Idle · frozen moon");
  });
});

describe("a byte count", () => {
  it("reads the way a person says it", () => {
    expect(humanBytes(512)).toBe("512 B");
    expect(humanBytes(640_000_000)).toBe("640 MB");
    expect(humanBytes(17_200_000_000)).toBe("17.2 GB");
  });
});
