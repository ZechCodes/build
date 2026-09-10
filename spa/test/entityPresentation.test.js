import { describe, it, expect } from "vitest";
import {
  RUN_STATE_LABEL,
  runChipClass,
  PLAN_STATE_LABEL,
  planChipClass,
  attnRuns,
  attnPlans,
} from "../src/core/entityPresentation.js";

describe("runChipClass", () => {
  it("parked states wear the warn palette (idle_unreported, interrupted)", () => {
    expect(runChipClass("idle_unreported")).toBe("warn");
    expect(runChipClass("interrupted")).toBe("warn");
  });

  it("blocked and failed stay warn", () => {
    expect(runChipClass("blocked")).toBe("warn");
    expect(runChipClass("failed")).toBe("warn");
  });

  it("review is attention, merged is done, building/created are work", () => {
    expect(runChipClass("review")).toBe("attn");
    expect(runChipClass("merged")).toBe("done");
    expect(runChipClass("building")).toBe("work");
    expect(runChipClass("created")).toBe("work");
  });
});

describe("planChipClass", () => {
  it("parked states wear the warn palette (idle_unreported, interrupted)", () => {
    expect(planChipClass("idle_unreported")).toBe("warn");
    expect(planChipClass("interrupted")).toBe("warn");
  });

  it("blocked and failed stay warn", () => {
    expect(planChipClass("blocked")).toBe("warn");
    expect(planChipClass("failed")).toBe("warn");
  });

  it("plan_review is attention, approved is done, drafting is work", () => {
    expect(planChipClass("plan_review")).toBe("attn");
    expect(planChipClass("approved")).toBe("done");
    expect(planChipClass("drafting")).toBe("work");
  });
});

describe("state labels survive the extraction", () => {
  it("keeps run labels", () => {
    expect(RUN_STATE_LABEL.review).toBe("READY TO REVIEW");
    expect(RUN_STATE_LABEL.idle_unreported).toBe("IDLE");
    expect(RUN_STATE_LABEL.interrupted).toBe("INTERRUPTED");
  });

  it("keeps plan labels", () => {
    expect(PLAN_STATE_LABEL.plan_review).toBe("READY TO REVIEW");
    expect(PLAN_STATE_LABEL.drafting).toBe("PLANNING");
  });
});

describe("attention filters", () => {
  it("attnRuns keeps needs-attention non-terminal runs", () => {
    const runs = [
      { run_id: "a", needs_attention: true, state: "review" },
      { run_id: "b", needs_attention: false, state: "review" },
      { run_id: "c", needs_attention: true, state: "merged" },
    ];
    expect(attnRuns(runs).map((r) => r.run_id)).toEqual(["a"]);
  });

  it("attnPlans keeps needs-attention non-terminal plans", () => {
    const plans = [
      { plan_id: "a", needs_attention: true, state: "plan_review" },
      { plan_id: "b", needs_attention: true, state: "abandoned" },
    ];
    expect(attnPlans(plans).map((p) => p.plan_id)).toEqual(["a"]);
  });
});
