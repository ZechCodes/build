// Closing out a branch: which surfaces offer it, what the two behaviors send,
// and what each confirmation promises. The bridge's `branch.finish` is the only
// way a branch leaves the board, so the model that decides when to offer it and
// what to say has to be right before any of it reaches the DOM.

import { describe, it, expect } from "vitest";
import {
  branchCloseout,
  branchFinishBlockReason,
  branchFinishConfirm,
  branchFinishParams,
  branchUnlinkConfirm,
  isUnlinkRefusal,
} from "../src/core/branchFinish.js";

const clean = { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" };

const branchRow = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  worktree_id: "wt-1",
  run_id: null,
  issue_id: null,
  primary: false,
  can_finish: true,
  stat: clean,
  ...over,
});

describe("whether a branch can be closed out here", () => {
  it("offers the control on a worktree-backed branch", () => {
    const closeout = branchCloseout(branchRow());
    expect(closeout.shown).toBe(true);
    expect(closeout.ready).toBe(true);
    expect(closeout.reason).toBe("");
    expect(closeout.options.map((option) => option.id)).toEqual(["finish_cleanup", "finish_delete"]);
  });

  it("offers the control on a run-backed branch", () => {
    const closeout = branchCloseout(branchRow({ run_id: "run-1", worktree_id: "wt-1" }));
    expect(closeout.shown).toBe(true);
  });

  // The primary checkout IS the repository: there is nothing to file away and
  // everything to lose, and the bridge refuses it. Never offer it.
  it("hides the control on a project's primary checkout", () => {
    expect(branchCloseout(branchRow({ primary: true, worktree_id: null })).shown).toBe(false);
    expect(branchCloseout(branchRow({ primary: true, run_id: "run-1" })).shown).toBe(false);
  });

  it("hides the control when no checkout on this device carries the branch", () => {
    expect(branchCloseout(null).shown).toBe(false);
    expect(branchCloseout(branchRow({ worktree_id: null, run_id: null })).shown).toBe(false);
  });

  // Shown but not ready: the user learns the branch CAN be closed out, and what
  // stands between them and it.
  it("shows the control disabled, with the reason, while work is only local", () => {
    const closeout = branchCloseout(branchRow({ can_finish: false, stat: { ...clean, ahead: 2 } }));
    expect(closeout.shown).toBe(true);
    expect(closeout.ready).toBe(false);
    expect(closeout.reason).toContain("2");
  });

  it("names the two options after the branch they act on", () => {
    const [keep, remove] = branchCloseout(branchRow()).options;
    expect(keep.description).toContain("build/login");
    expect(remove.description).toContain("build/login");
  });
});

describe("why a branch is not finishable yet", () => {
  it("asks for a commit first, counting what is uncommitted", () => {
    const reason = branchFinishBlockReason(branchRow({ stat: { ...clean, uncommitted: { files_changed: 1 } } }));
    expect(reason).toContain("1 uncommitted file");
    expect(reason).not.toContain("files");
  });

  it("asks for an upstream when the branch has none", () => {
    expect(branchFinishBlockReason(branchRow({ stat: { ...clean, upstream: null } }))).toContain("Push");
  });

  it("asks for a push, counting the commits only this machine has", () => {
    expect(branchFinishBlockReason(branchRow({ stat: { ...clean, ahead: 3 } }))).toContain("3 unpushed commits");
  });

  it("still says something when the row carries no stat at all", () => {
    expect(branchFinishBlockReason({})).not.toBe("");
  });
});

describe("what each option sends", () => {
  it("keeps the branch on the default option", () => {
    expect(branchFinishParams("finish_cleanup", { projectId: "p1", branch: "build/login" })).toEqual({
      project_id: "p1",
      branch: "build/login",
      action: "cleanup",
    });
  });

  it("deletes the branch on the second option", () => {
    expect(branchFinishParams("finish_delete", { projectId: "p1", branch: "build/login" }).action).toBe("delete");
  });

  // The override the bridge's own refusal names: finish the branch, leave the
  // issue it implements open.
  it("carries unlink only when it was asked for", () => {
    const params = branchFinishParams("finish_cleanup", { projectId: "p1", branch: "b", unlink: true });
    expect(params.unlink).toBe(true);
    expect(branchFinishParams("finish_cleanup", { projectId: "p1", branch: "b" }).unlink).toBeUndefined();
  });

  it("refuses an option it has no action for", () => {
    expect(() => branchFinishParams("merge", { projectId: "p1", branch: "b" })).toThrow(/unknown/);
  });
});

describe("what the confirmation promises", () => {
  it("outlines keeping the branch", () => {
    const plan = branchFinishConfirm("finish_cleanup", { branch: "build/login" });
    expect(plan.actions).toEqual(["Remove the checkout for build/login", "Keep branch build/login"]);
    expect(plan.danger).toBe(false);
  });

  it("outlines deleting the branch, as the destructive verb it is", () => {
    const plan = branchFinishConfirm("finish_delete", { branch: "build/login" });
    expect(plan.actions).toContain("Delete branch build/login");
    expect(plan.danger).toBe(true);
  });

  // Done on a branch that implements an issue archives the issue with it — the
  // outline says so before the click, not after.
  it("says the issue goes with it when the branch implements one", () => {
    const plan = branchFinishConfirm("finish_cleanup", { branch: "build/login", issueId: "issue-1" });
    expect(plan.actions.some((action) => action.includes("issue"))).toBe(true);
  });

  it("leaves the issue out when the branch implements none", () => {
    const plan = branchFinishConfirm("finish_cleanup", { branch: "build/login" });
    expect(plan.actions.some((action) => action.includes("issue"))).toBe(false);
  });
});

describe("the unlink override", () => {
  it("recognizes the bridge's refusal by the flag it names", () => {
    expect(isUnlinkRefusal("branch.finish: … pass unlink to finish the branch alone")).toBe(true);
    expect(isUnlinkRefusal("worktree.finish cleanup requires no uncommitted changes")).toBe(false);
    expect(isUnlinkRefusal(null)).toBe(false);
  });

  it("hands the bridge's own words back as the disclosure", () => {
    const plan = branchUnlinkConfirm("build/login", "pass unlink to finish the branch alone");
    expect(plan.intro).toBe("pass unlink to finish the branch alone");
    expect(plan.actions[0]).toContain("build/login");
  });
});
