import { describe, it, expect } from "vitest";
import { planBucketKey, bucketPlans } from "../src/core/planRail.js";
import { planStageBoardHtml } from "../src/views/planStages.js";
import { canImplement, implementBlockReason } from "../src/core/taskActions.js";

describe("planBucketKey", () => {
  it("puts a plan still being authored in Draft", () => {
    expect(planBucketKey("created")).toBe("draft");
    expect(planBucketKey("drafting")).toBe("draft");
  });

  it("puts the review gate and every parked arm in In review", () => {
    for (const s of ["plan_review", "blocked", "failed", "idle_unreported", "interrupted"]) {
      expect(planBucketKey(s)).toBe("review");
    }
  });

  it("puts an approved plan in Approved", () => {
    expect(planBucketKey("approved")).toBe("approved");
  });

  it("puts the terminal (abandoned) plan in History", () => {
    expect(planBucketKey("abandoned")).toBe("history");
  });
});

describe("bucketPlans", () => {
  it("groups a mixed list into ordered rail buckets", () => {
    const plans = [
      { plan_id: "a", state: "drafting" },
      { plan_id: "b", state: "plan_review" },
      { plan_id: "c", state: "approved" },
      { plan_id: "d", state: "abandoned" },
      { plan_id: "e", state: "blocked" },
    ];
    const b = bucketPlans(plans);
    expect(b.draft.map((p) => p.plan_id)).toEqual(["a"]);
    expect(b.review.map((p) => p.plan_id)).toEqual(["b", "e"]);
    expect(b.approved.map((p) => p.plan_id)).toEqual(["c"]);
    expect(b.history.map((p) => p.plan_id)).toEqual(["d"]);
  });

  it("is safe for an empty/absent list", () => {
    expect(bucketPlans([])).toEqual({ draft: [], review: [], approved: [], history: [] });
    expect(bucketPlans(undefined)).toEqual({ draft: [], review: [], approved: [], history: [] });
  });
});

describe("planStageBoardHtml", () => {
  const allPlanned = {
    stages: [
      { id: "s1", title: "Schema", summary: "add the column", state: "planned", open_comments: 0 },
      { id: "s2", title: "Backfill", summary: "", state: "planned", open_comments: 2 },
    ],
  };
  const partlyApproved = {
    stages: [
      { id: "s1", title: "Schema", summary: "", state: "approved", open_comments: 0 },
      { id: "s2", title: "Backfill", summary: "", state: "planned", open_comments: 0 },
    ],
  };

  it("keeps id=stagelist so the poll freeze check can find it", () => {
    expect(planStageBoardHtml({ state: "plan_review" }, allPlanned)).toContain('id="stagelist"');
  });

  it("offers Approve-all only while every stage is still planned", () => {
    expect(planStageBoardHtml({ state: "plan_review" }, allPlanned)).toContain('id="approveall"');
    expect(planStageBoardHtml({ state: "plan_review" }, partlyApproved)).not.toContain('id="approveall"');
  });

  it("renders each stage title with its plan-side doc-state chip", () => {
    const html = planStageBoardHtml({ state: "plan_review" }, partlyApproved);
    expect(html).toContain("Schema");
    expect(html).toContain("Backfill");
    expect(html).toContain("APPROVED");
    expect(html).toContain("PLANNED");
  });

  it("shows the open-comment badge only when a stage carries open comments", () => {
    expect(planStageBoardHtml({ state: "plan_review" }, allPlanned)).toContain("2 💬");
    expect(planStageBoardHtml({ state: "plan_review" }, partlyApproved)).not.toContain("💬");
  });

  it("renders a summary line when present and omits it otherwise", () => {
    const html = planStageBoardHtml({ state: "plan_review" }, allPlanned);
    expect(html).toContain("add the column");
    expect(html).toContain('class="stagesummary"');
  });

  it("shows an empty-state row when the manifest has no stages", () => {
    expect(planStageBoardHtml({ state: "plan_review" }, { stages: [] })).toContain("No stages yet.");
  });

  it("never shows a run-side control (no run-all, no review banner) — plan side is docs only", () => {
    const html = planStageBoardHtml({ state: "plan_review" }, allPlanned);
    expect(html).not.toContain('id="runall"');
    expect(html).not.toContain("stage-validation");
  });
});

// Implement-availability text drives the plan cockpit's footer (enabled split
// button vs a disabled button carrying the bridge's own rejection reason).
describe("Implement availability text (plan cockpit footer)", () => {
  const readyStages = [{ id: "s1", state: "approved" }];

  it("is available with no reason when approved, first stage approved, and no run", () => {
    const plan = { state: "approved", active_run_id: null, stages: readyStages };
    expect(canImplement(plan)).toBe(true);
    expect(implementBlockReason(plan)).toBeNull();
  });

  it("explains the not-yet-approved plan at the review gate", () => {
    const plan = { state: "plan_review", active_run_id: null, stages: readyStages };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/approve the plan/i);
  });

  it("explains the unapproved first stage", () => {
    const plan = { state: "approved", active_run_id: null, stages: [{ id: "s1", state: "planned" }] };
    expect(implementBlockReason(plan)).toMatch(/first stage/i);
  });

  it("explains a run already in progress", () => {
    const plan = { state: "approved", active_run_id: "run-9", stages: readyStages };
    expect(implementBlockReason(plan)).toMatch(/already implementing/i);
  });

  // Single-doc plans (and migrated legacy single-plan tasks) carry an EMPTY
  // stages array. The bridge only gates the first stage doc when one exists, so
  // an empty manifest imposes no first-stage requirement — only the plan-approval
  // and single-active-writer gates remain.
  it("is available for an approved single-doc plan (empty stages, no run)", () => {
    const plan = { state: "approved", active_run_id: null, stages: [] };
    expect(canImplement(plan)).toBe(true);
    expect(implementBlockReason(plan)).toBeNull();
  });

  it("still blocks an unapproved single-doc plan at the review gate", () => {
    const plan = { state: "plan_review", active_run_id: null, stages: [] };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/approve the plan/i);
  });

  it("still blocks a single-doc plan that already has a run", () => {
    const plan = { state: "approved", active_run_id: "run-3", stages: [] };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/already implementing/i);
  });
});
