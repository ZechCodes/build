// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { planBucketKey, bucketPlans } from "../src/core/planRail.js";
import { planStageBoardHtml, docErrorPaneHtml, renderPlanStages } from "../src/views/planStages.js";
import { issueThreadLinkTarget } from "../src/views/plan.js";
import { planReviewSkeletonHtml } from "../src/core/planReview.js";
import { canImplement, implementBlockReason, shouldFetchPlanDoc, planDocPaneState } from "../src/core/taskActions.js";

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

  it("joins stage-plan approval and execution and offers Implement all on a ready Issue", () => {
    const joined = {
      auto_advance: false,
      stages: [
        { id: "s1", title: "Schema", state: "approved", approval: "approved", execution: "complete", start_sha: "base", completion_sha: "abc", publication: "local" },
        { id: "s2", title: "Backfill", state: "approved", approval: "approved", execution: "pending" },
      ],
    };
    const html = planStageBoardHtml({ state: "approved" }, joined);
    expect(html).toContain('id="implementall"');
    expect(html).toContain("COMPLETE");
    expect(html).toContain("abc");
    expect(html).toContain("local");
  });
});

describe("Issue conversation typed links", () => {
  it("routes file references into the active implementation's scoped Files surface", () => {
    expect(issueThreadLinkTarget(
      { kind: "file", path: "src/parser.js" },
      { project_id: "p1", current_implementation_id: "run-1" },
      "fallback",
    )).toEqual({
      route: { name: "task", projectId: "p1", id: "run-1", tab: "files" },
      filePath: "src/parser.js",
    });
  });
});

describe("plan review layout", () => {
  it("keeps the plan artifact skeleton free of the dedicated conversation", () => {
    const html = planReviewSkeletonHtml();
    expect(html).toContain('id="planbody"');
    expect(html).not.toContain('id="planthread"');
  });

  it("uses the shared conversation instead of a form below a stage document", () => {
    document.body.innerHTML = '<div id="planbody"></div>';
    const stage = { id: "s1", title: "Schema", state: "planned", open_comments: 0, comments: [] };
    renderPlanStages({
      body: document.querySelector("#planbody"),
      plan: { plan_id: "p1", state: "plan_review" },
      stagesData: { stages: [stage] },
      stageDoc: { stage_id: "s1", contents: "# Schema" },
      stageDocState: "ready",
      selectedStageId: "s1",
      callRpc: async () => {},
      repaint: () => {},
      onSelectStage: () => {},
    });
    expect(document.querySelector("#stagedoc")).not.toBeNull();
    expect(document.querySelector("#stage-general")).toBeNull();
    expect(document.querySelector(".plan-feedback")).toBeNull();
  });

  it("offers Implement stage for an approved pending stage plan", () => {
    document.body.innerHTML = '<div id="planbody"></div>';
    const stage = { id: "s1", title: "Schema", state: "approved", approval: "approved", execution: "pending", open_comments: 0, comments: [] };
    renderPlanStages({
      body: document.querySelector("#planbody"),
      plan: { issue_id: "p1", plan_id: "p1", state: "approved" },
      stagesData: { stages: [stage], auto_advance: false },
      stageDoc: { stage_id: "s1", contents: "# Schema" },
      stageDocState: "ready",
      selectedStageId: "s1",
      callRpc: async () => {},
      repaint: () => {},
      onSelectStage: () => {},
    });
    expect(document.querySelector("#implementstage")).not.toBeNull();
  });

  it("renders escaped invalidation details and offers the legal stage repair action", async () => {
    document.body.innerHTML = '<div id="planbody"></div>';
    const calls = [];
    const stage = {
      id: "s1",
      title: "Schema",
      state: "approved",
      approval: "approved",
      execution: "validation_failed",
      invalidation_reason: '<img src=x onerror="globalThis.pwned=1">',
      validation: { passed: false, findings: "Migration missing" },
      open_comments: 0,
      comments: [],
    };
    renderPlanStages({
      body: document.querySelector("#planbody"),
      plan: { issue_id: "p1", plan_id: "p1", state: "approved" },
      stagesData: { stages: [stage] },
      stageDoc: { stage_id: "s1", contents: "# Schema" },
      stageDocState: "ready",
      selectedStageId: "s1",
      callRpc: async (method, params) => calls.push([method, params]),
      repaint: () => {},
      onSelectStage: () => {},
    });
    expect(document.querySelector("#fixstage")).not.toBeNull();
    expect(document.querySelector("#planbody img")).toBeNull();
    expect(document.querySelector("#planbody").textContent).toContain('<img src=x onerror="globalThis.pwned=1">');
    document.querySelector("#fixstage").click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(calls).toEqual([["issue.stage_fix", { issue_id: "p1", stage_id: "s1", note: "" }]]);
  });

  it("escapes unknown status fallbacks from malformed compatibility payloads", () => {
    const attack = '<img src=x onerror="globalThis.pwned=1">';
    const html = planStageBoardHtml(
      { state: "approved", implementation_activity: "idle" },
      { stages: [{ id: "s1", title: "Schema", state: attack, approval: attack, execution: attack, open_comments: attack }] },
    );
    document.body.innerHTML = html;
    expect(document.querySelector("img")).toBeNull();
    expect(document.body.textContent).toContain(attack);
  });

  it("offers stable diff for a completed stage", () => {
    document.body.innerHTML = '<div id="planbody"></div>';
    const stage = { id: "s1", title: "Schema", state: "approved", approval: "approved", execution: "complete", start_sha: "base", completion_sha: "abc", open_comments: 0, comments: [] };
    renderPlanStages({
      body: document.querySelector("#planbody"),
      plan: { plan_id: "p1", state: "plan_review" },
      stagesData: { stages: [stage] },
      stageDoc: { stage_id: "s1", contents: "# Schema" },
      stageDocState: "ready",
      selectedStageId: "s1",
      callRpc: async () => {},
      repaint: () => {},
      onSelectStage: () => {},
    });
    expect(document.querySelector("#stagediff")).not.toBeNull();
  });
});

// The doc-read error pane carries an inline Retry affordance (W15) instead of the
// old "Reopen the plan/stage to retry" copy. The button ids let plan.js /
// planStages.js wire the latch-clear + refetch.
describe("docErrorPaneHtml (doc-read Retry)", () => {
  it("offers an inline Retry button for the plan doc", () => {
    const html = docErrorPaneHtml("plan");
    expect(html).toContain('class="plan-empty warn"');
    expect(html).toContain('id="docretry"');
    expect(html).toContain("Retry");
    expect(html).toContain("the stage plan document");
    expect(html).not.toContain("Reopen");
  });

  it("offers an inline Retry button for a stage doc with a distinct id", () => {
    const html = docErrorPaneHtml("stage");
    expect(html).toContain('id="stagedocretry"');
    expect(html).toContain("Retry");
    expect(html).toContain("this stage document");
    expect(html).not.toContain("Reopen");
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
    expect(implementBlockReason(plan)).toMatch(/mark the issue ready/i);
  });

  it("explains the unapproved first stage", () => {
    const plan = { state: "approved", active_run_id: null, stages: [{ id: "s1", state: "planned" }] };
    expect(implementBlockReason(plan)).toMatch(/first stage/i);
  });

  it("explains a run already in progress", () => {
    const plan = { state: "approved", active_run_id: "run-9", stages: readyStages };
    expect(implementBlockReason(plan)).toMatch(/implementation is already active/i);
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
    expect(implementBlockReason(plan)).toMatch(/mark the issue ready/i);
  });

  it("still blocks a single-doc plan that already has a run", () => {
    const plan = { state: "approved", active_run_id: "run-3", stages: [] };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/implementation is already active/i);
  });

  // A migrated plan whose canonical docs are gone can never be materialized —
  // the block precedes every state gate, even for an otherwise-ready plan.
  it("blocks a plan whose docs are unavailable, with a reason, ahead of the state gates", () => {
    const plan = { state: "approved", active_run_id: null, stages: [], docs_available: false };
    expect(canImplement(plan)).toBe(false);
    expect(implementBlockReason(plan)).toMatch(/stage plans are unavailable/i);
  });

  it("still allows implement when docs_available is true or absent", () => {
    expect(canImplement({ state: "approved", active_run_id: null, stages: [], docs_available: true })).toBe(true);
    expect(canImplement({ state: "approved", active_run_id: null, stages: [] })).toBe(true);
  });
});

// The doc-fetch guard and pane-state decision that fix the "loading forever"
// bug: docs that predate canonical storage (docs_available false) and docs whose
// read has errored (latched) are never refetched, and the pane renders an honest
// state instead of the loading placeholder.
describe("plan doc fetch guard (shouldFetchPlanDoc)", () => {
  it("fetches when docs are available and no read has errored", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: true, errorLatched: false })).toBe(true);
    expect(shouldFetchPlanDoc({ docsAvailable: undefined, errorLatched: false })).toBe(true);
  });

  it("never fetches a doc that predates canonical storage", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: false, errorLatched: false })).toBe(false);
  });

  it("never refetches a doc whose read has errored (latched off)", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: true, errorLatched: true })).toBe(false);
  });
});

describe("plan doc pane state (planDocPaneState)", () => {
  it("is unavailable when the docs predate canonical storage — ahead of any error/contents", () => {
    expect(planDocPaneState({ docsAvailable: false, errorLatched: false, hasContents: false })).toBe("unavailable");
    expect(planDocPaneState({ docsAvailable: false, errorLatched: true, hasContents: true })).toBe("unavailable");
  });

  it("is error when a read has errored and is latched", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: true, hasContents: false })).toBe("error");
  });

  it("is ready when contents are in hand", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: false, hasContents: true })).toBe("ready");
  });

  it("is loading while still awaiting the first successful read", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: false, hasContents: false })).toBe("loading");
  });
});
