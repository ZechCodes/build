import { describe, it, expect } from "vitest";
import {
  canDelete,
  canAbandon,
  mergeFailureReason,
  bannerText,
  planDeletable,
  canImplement,
  implementBlockReason,
  defaultRunTab,
  stageNotesTarget,
  gitActionConfirm,
  abandonConfirm,
  deleteRunConfirm,
  deletePlanConfirm,
  approvePlanConfirm,
  implementConfirm,
  planBackTarget,
} from "../src/core/taskActions.js";

// Live (non-deletable) run states, from the wire contract.
const LIVE_RUN_STATES = ["created", "building", "stage_gate", "review", "blocked", "idle_unreported", "interrupted"];
const TERMINAL_RUN_STATES = ["merged", "abandoned", "archived", "failed"];

describe("run removal actions match the bridge contract", () => {
  it("delete is offered only for terminal states (merged/abandoned/archived/failed)", () => {
    for (const s of TERMINAL_RUN_STATES) expect(canDelete(s)).toBe(true);
    for (const s of LIVE_RUN_STATES) expect(canDelete(s)).toBe(false);
  });

  it("abandon is offered for every live, non-deletable state", () => {
    for (const s of LIVE_RUN_STATES) expect(canAbandon(s)).toBe(true);
    for (const s of TERMINAL_RUN_STATES) expect(canAbandon(s)).toBe(false);
  });

  it("delete and abandon are mutually exclusive (exactly one removal action)", () => {
    for (const s of [...LIVE_RUN_STATES, "merged", "abandoned"]) expect(canDelete(s)).not.toBe(canAbandon(s));
  });

  it("abandon is not offered for a missing/unknown state", () => {
    expect(canAbandon("")).toBe(false);
    expect(canAbandon(undefined)).toBe(false);
  });
});

describe("plan removal actions match the bridge contract", () => {
  const LIVE_PLAN_STATES = ["created", "drafting", "plan_review", "approved", "blocked", "failed", "idle_unreported", "interrupted"];

  it("plan delete is offered only for the terminal abandoned state", () => {
    expect(planDeletable("abandoned")).toBe(true);
    for (const s of LIVE_PLAN_STATES) expect(planDeletable(s)).toBe(false);
  });

  it("has no live Issue Abandon helper; only historical abandoned records can be deleted", () => {
    for (const s of LIVE_PLAN_STATES) expect(planDeletable(s)).toBe(false);
  });
});

describe("planBackTarget — the plan chevron's return route", () => {
  it("returns to the originating run's stages tab when the marker matches the active run", () => {
    expect(planBackTarget({ returnRunId: "run-7", activeRunId: "run-7", projectId: "p1" })).toEqual({
      name: "task",
      projectId: "p1",
      id: "run-7",
      tab: "stages",
    });
  });

  it("falls back to the project when the return marker is stale (no longer the active run)", () => {
    expect(planBackTarget({ returnRunId: "run-old", activeRunId: "run-7", projectId: "p1" })).toEqual({
      name: "project",
      projectId: "p1",
    });
  });

  it("falls back to the project when there is no return marker", () => {
    expect(planBackTarget({ returnRunId: null, activeRunId: "run-7", projectId: "p1" })).toEqual({
      name: "project",
      projectId: "p1",
    });
  });

  it("falls back to notifications when there is no project", () => {
    expect(planBackTarget({ returnRunId: null, activeRunId: null, projectId: null })).toEqual({ name: "notifications" });
  });

  it("does not return to a run when there is a marker but no active run (run gone)", () => {
    expect(planBackTarget({ returnRunId: "run-7", activeRunId: null, projectId: "p1" })).toEqual({
      name: "project",
      projectId: "p1",
    });
  });
});

describe("Implement availability", () => {
  const approvedFirstStage = [{ id: "s1", state: "approved" }, { id: "s2", state: "planned" }];

  it("is available when approved, the first stage doc is approved, and no active run", () => {
    const plan = { state: "approved", active_run_id: null, stages: approvedFirstStage };
    expect(canImplement(plan)).toBe(true);
    expect(implementBlockReason(plan)).toBeNull();
  });

  it("is blocked (with a reason) while the plan is not yet approved", () => {
    const plan = { state: "plan_review", active_run_id: null, stages: approvedFirstStage };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/mark the issue ready/i);
  });

  it("is blocked while the first stage doc is not approved", () => {
    const plan = { state: "approved", active_run_id: null, stages: [{ id: "s1", state: "planned" }] };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/stage/i);
  });

  it("is blocked while a run already implements the plan (single active writer)", () => {
    const plan = { state: "approved", active_run_id: "run-7", stages: approvedFirstStage };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/implementation/i);
  });

  it("is unavailable for a missing plan", () => {
    expect(canImplement(null)).toBe(false);
  });

  it("is available for a single-doc / migrated plan (empty stages: no first-stage gate)", () => {
    // The bridge only gates the first stage doc when one exists; an empty stages
    // array (single-doc or legacy migrated plan) imposes no per-stage gate.
    const plan = { state: "approved", active_run_id: null, stages: [] };
    expect(canImplement(plan)).toBe(true);
    expect(implementBlockReason(plan)).toBeNull();
  });

  it("is still gated by plan approval and the single-active-writer rule when stages is empty", () => {
    expect(canImplement({ state: "plan_review", active_run_id: null, stages: [] })).toBe(false);
    expect(implementBlockReason({ state: "plan_review", active_run_id: null, stages: [] })).toMatch(/mark the issue ready/i);
    expect(canImplement({ state: "approved", active_run_id: "run-1", stages: [] })).toBe(false);
    expect(implementBlockReason({ state: "approved", active_run_id: "run-1", stages: [] })).toMatch(/implementation is already active/i);
  });

  it("is unavailable for a plan whose stages array is absent altogether (no gate, but approval still required)", () => {
    expect(canImplement({ state: "approved", active_run_id: null })).toBe(true);
  });
});

describe("stage-notes routing (which send-notes verb applies)", () => {
  it("routes through the plan while it is still under review", () => {
    expect(stageNotesTarget({ state: "plan_review", issue_id: "pl-1", plan_id: "pl-1", active_run_id: null })).toEqual({
      method: "issue.stage_revise",
      entityId: "pl-1",
    });
  });

  it("routes through the active run once the plan is approved (plan docs are frozen)", () => {
    expect(stageNotesTarget({ state: "approved", plan_id: "pl-1", active_run_id: "run-9" })).toEqual({
      method: "run.stage_send_notes",
      entityId: "run-9",
    });
  });

  it("has no revision path for an approved plan with no run", () => {
    expect(stageNotesTarget({ state: "approved", plan_id: "pl-1", active_run_id: null })).toBeNull();
  });

  it("is safe for a missing plan", () => {
    expect(stageNotesTarget(null)).toBeNull();
  });
});

describe("default run tab selection", () => {
  it("opens a between-stages run (stage_gate) on Stages", () => {
    expect(defaultRunTab({ state: "stage_gate" })).toBe("stages");
  });

  it("opens every other run state on Changes (the review diff)", () => {
    for (const s of ["created", "building", "review", "blocked", "failed", "idle_unreported", "interrupted", "merged", "abandoned", "archived"]) {
      expect(defaultRunTab({ state: s })).toBe("changes");
    }
  });

  it("is safe for a missing run", () => {
    expect(defaultRunTab(null)).toBe("changes");
    expect(defaultRunTab(undefined)).toBe("changes");
  });
});

describe("git-action confirmation plans (the modal's step outline)", () => {
  const names = { branch: "feat/x", base: "main" };

  it("merge & clean up outlines commit, merge, worktree removal, and branch deletion", () => {
    expect(gitActionConfirm("merge_prune", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Delete the worktree",
        "Delete branch feat/x",
      ],
      confirmLabel: "Merge & clean up",
      danger: true,
    });
  });

  it("merge & keep outlines commit + merge and keeps the worktree (not danger)", () => {
    expect(gitActionConfirm("merge_keep", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Keep the worktree and branch",
      ],
      confirmLabel: "Merge",
      danger: false,
    });
  });

  it("merge & release outlines the un-adopt step (not danger)", () => {
    expect(gitActionConfirm("merge_release", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Release the task — keep the worktree and branch, drop the task",
      ],
      confirmLabel: "Merge & release",
      danger: false,
    });
  });

  it("merge & push appends the origin push to the clean-up steps", () => {
    expect(gitActionConfirm("merge_push", names)).toEqual({
      title: "Merge feat/x?",
      actions: [
        "Commit any uncommitted changes on feat/x",
        "Merge feat/x into main",
        "Delete the worktree",
        "Delete branch feat/x",
        "Push main to origin",
      ],
      confirmLabel: "Merge & push",
      danger: true,
    });
  });

  it("non-destructive actions (commit, push) get NO modal", () => {
    expect(gitActionConfirm("commit", names)).toBeNull();
    expect(gitActionConfirm("push", names)).toBeNull();
    expect(gitActionConfirm("unknown_option", names)).toBeNull();
  });

  it("interpolates a placeholder base ('the base branch') gracefully", () => {
    const plan = gitActionConfirm("merge_push", { branch: "feat/x", base: "the base branch" });
    expect(plan.actions).toContain("Merge feat/x into the base branch");
    expect(plan.actions).toContain("Push the base branch to origin");
  });
});

describe("abandon confirmation plan", () => {
  it("an adopted worktree warns that files Build did not create are deleted", () => {
    expect(abandonConfirm({ adopted: true, branch: "feat/x" })).toEqual({
      title: "Delete this adopted worktree?",
      actions: [
        "Delete the worktree (files Build did not create)",
        "Delete branch feat/x",
        "Keep the task as history",
      ],
      confirmLabel: "Delete worktree",
      danger: true,
    });
  });

  it("a Build-created task gets the plain abandon outline", () => {
    expect(abandonConfirm({ adopted: false, branch: "feat/x" })).toEqual({
      title: "Abandon this task?",
      actions: ["Delete the worktree", "Delete branch feat/x", "Keep the task as history"],
      confirmLabel: "Abandon",
      danger: true,
    });
  });
});

describe("run/plan removal confirmation plans", () => {
  it("deleting a run removes the record (danger)", () => {
    expect(deleteRunConfirm()).toEqual({
      title: "Delete this task?",
      actions: ["Remove the task record permanently"],
      confirmLabel: "Delete",
      danger: true,
    });
  });

  it("deleting a plan removes the record (danger)", () => {
    expect(deletePlanConfirm()).toEqual({
      title: "Delete this issue?",
      actions: ["Remove the Issue record and stage plans permanently"],
      confirmLabel: "Delete",
      danger: true,
    });
  });
});

describe("plan gate confirmation plans (Approve / Implement)", () => {
  it("approve outlines the worktree teardown and the implement unlock", () => {
    expect(approvePlanConfirm()).toEqual({
      title: "Mark this issue ready?",
      actions: [
        "The planning worktree is removed — the Issue stage plans are already saved",
        "Implementation unlocks for approved stage plans",
      ],
      confirmLabel: "Mark ready",
      danger: false,
    });
  });

  it("implement outlines the worktree, the agent session, and the notification", () => {
    expect(implementConfirm({ base: "main" })).toEqual({
      title: "Implement this issue?",
      intro: "A fresh agent session will execute the approved stage plans.",
      actions: [
        "Create a worktree on a new branch off main",
        "Start a coding agent session for the approved stage plans",
        "You'll be notified when it's ready to review",
      ],
      confirmLabel: "Implement",
      danger: false,
    });
  });

  it("implement interpolates a placeholder base gracefully", () => {
    expect(implementConfirm({ base: "the base branch" }).actions[0]).toBe(
      "Create a worktree on a new branch off the base branch",
    );
  });

  it("implement outlines the checkout it was targeted at instead of cutting one", () => {
    expect(implementConfirm({ base: "main", branch: "feature-x" }).actions[0]).toBe(
      "Commit the stage plans onto feature-x as the review baseline",
    );
  });
});

describe("merge failure reason extraction", () => {
  it("strips the merge_failed: prefix and trims", () => {
    expect(mergeFailureReason("merge_failed: conflict in a.txt, b.txt")).toBe("conflict in a.txt, b.txt");
    expect(mergeFailureReason("merge_failed:primary checkout is on \"x\"")).toBe('primary checkout is on "x"');
  });

  it("returns null for non-merge errors", () => {
    expect(mergeFailureReason("unknown run_id")).toBeNull();
    expect(mergeFailureReason(null)).toBeNull();
    expect(mergeFailureReason(undefined)).toBeNull();
  });
});

describe("error banner precedence", () => {
  it("a held local error wins over the polled last_error", () => {
    expect(bannerText("error: run store io", null)).toBe("error: run store io");
    expect(bannerText("error: run store io", "merge_failed: x")).toBe("error: run store io");
  });

  it("falls back to the polled last_error when there is no local error", () => {
    expect(bannerText(null, "merge_failed: conflict")).toBe("merge_failed: conflict");
  });

  it("is empty (banner hidden) when neither is set", () => {
    expect(bannerText(null, null)).toBe("");
    expect(bannerText(null, undefined)).toBe("");
  });
});
