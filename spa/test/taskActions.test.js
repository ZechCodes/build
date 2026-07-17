import { describe, it, expect } from "vitest";
import {
  canDelete,
  canAbandon,
  mergeFailureReason,
  bannerText,
  planDeletable,
  planAbandonable,
  canImplement,
  implementBlockReason,
  defaultRunTab,
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

  it("plan abandon is offered for every non-terminal state", () => {
    for (const s of LIVE_PLAN_STATES) expect(planAbandonable(s)).toBe(true);
    expect(planAbandonable("abandoned")).toBe(false);
  });

  it("plan delete and abandon are mutually exclusive", () => {
    for (const s of [...LIVE_PLAN_STATES, "abandoned"]) expect(planDeletable(s)).not.toBe(planAbandonable(s));
  });

  it("plan abandon is not offered for a missing/unknown state", () => {
    expect(planAbandonable("")).toBe(false);
    expect(planAbandonable(undefined)).toBe(false);
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
    expect(implementBlockReason(plan)).toMatch(/approve/i);
  });

  it("is blocked while the first stage doc is not approved", () => {
    const plan = { state: "approved", active_run_id: null, stages: [{ id: "s1", state: "planned" }] };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/stage/i);
  });

  it("is blocked while a run already implements the plan (single active writer)", () => {
    const plan = { state: "approved", active_run_id: "run-7", stages: approvedFirstStage };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/run/i);
  });

  it("is unavailable for a missing plan or one with no stages", () => {
    expect(canImplement(null)).toBe(false);
    expect(canImplement({ state: "approved", active_run_id: null, stages: [] })).toBe(false);
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
