import { describe, it, expect } from "vitest";
import { STAGE_LABEL, stageChipClass, stageBoardHtml, runAllControlKind } from "../src/views/stages.js";

const WIRE_STATES = ["planned", "approved", "building", "built", "validating", "validated_passed", "validated_failed"];

describe("STAGE_LABEL", () => {
  it("labels every wire stage state", () => {
    for (const s of WIRE_STATES) expect(typeof STAGE_LABEL[s]).toBe("string");
  });
  it("collapses the validated pair into human copy", () => {
    expect(STAGE_LABEL.validated_passed).toBe("VALIDATED");
    expect(STAGE_LABEL.validated_failed).toBe("VALIDATION FAILED");
  });
});

describe("stageBoardHtml", () => {
  const validatedStages = [
    {
      id: "first",
      title: "First",
      summary: "",
      state: "validated_passed",
      open_comments: 0,
      validation: { passed: true, findings: "- schema matches the doc", notes_for_next_stage: "watch the rename" },
    },
    {
      id: "second",
      title: "Second",
      summary: "",
      state: "validated_passed",
      open_comments: 0,
      validation: { passed: true, findings: "final stage verified end to end", notes_for_next_stage: "" },
    },
  ];

  it("renders the list container with id=stagelist so the poll freeze check can find it", () => {
    const html = stageBoardHtml({ state: "plan_review" }, { stages: validatedStages, auto_advance: false });
    expect(html).toContain('id="stagelist"');
  });

  it("shows the final stage's findings as the merge decision context at the review gate", () => {
    // Spec §8.4: heading + FINDINGS. notes_for_next_stage is "" on a final
    // stage (there is no next stage), so keying the body on it would render
    // a heading-only banner and hide the merge decision context entirely.
    const html = stageBoardHtml({ state: "review" }, { stages: validatedStages, auto_advance: false });
    expect(html).toContain("passed");
    expect(html).toContain("final stage verified end to end");
  });

  it("renders no review banner outside the review gate", () => {
    const html = stageBoardHtml({ state: "plan_review" }, { stages: validatedStages, auto_advance: false });
    expect(html).not.toContain("stage-validation");
  });

  it("mounts a #runall host div (not a passive checkbox) for the run-all control", () => {
    const html = stageBoardHtml({ state: "plan_review" }, { stages: validatedStages, auto_advance: false });
    expect(html).toContain('id="runall"');
    expect(html).toContain('class="runall"');
    // the old passive checkbox is gone — the control is mounted by the wiring
    expect(html).not.toContain('type="checkbox" id="runall"');
  });
});

describe("runAllControlKind", () => {
  const planned = [
    { id: "a", state: "planned" },
    { id: "b", state: "planned" },
  ];
  const someApproved = [
    { id: "a", state: "approved" },
    { id: "b", state: "planned" },
  ];
  const midRun = [
    { id: "a", state: "validated_passed" },
    { id: "b", state: "building" },
  ];
  const allDone = [
    { id: "a", state: "validated_passed" },
    { id: "b", state: "validated_passed" },
  ];

  it("shows the run split-button on a fresh all-planned board", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: planned, taskState: "plan_review" })).toBe("run");
  });

  it("shows the run control when some stages are already approved", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: someApproved, taskState: "plan_review" })).toBe("run");
  });

  it("still shows the run control mid-run (unfinished stages remain)", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: midRun, taskState: "building" })).toBe("run");
  });

  it("shows the stop control whenever auto-advance is already on", () => {
    expect(runAllControlKind({ autoAdvance: true, stages: planned, taskState: "plan_review" })).toBe("stop");
    expect(runAllControlKind({ autoAdvance: true, stages: allDone, taskState: "review" })).toBe("stop");
  });

  it("shows nothing once every stage has validated (nothing left to run)", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: allDone, taskState: "review" })).toBe("none");
  });

  it("shows nothing on a terminal task", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: planned, taskState: "merged" })).toBe("none");
    expect(runAllControlKind({ autoAdvance: false, stages: planned, taskState: "abandoned" })).toBe("none");
  });

  it("shows nothing when there are no stages", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: [], taskState: "plan_review" })).toBe("none");
  });
});

describe("stageChipClass", () => {
  it("maps each wire state to a chip palette class", () => {
    expect(stageChipClass("planned")).toBe("");
    expect(stageChipClass("approved")).toBe("attn");
    expect(stageChipClass("building")).toBe("work");
    expect(stageChipClass("built")).toBe("work");
    expect(stageChipClass("validating")).toBe("work");
    expect(stageChipClass("validated_passed")).toBe("done");
    expect(stageChipClass("validated_failed")).toBe("warn");
  });
  it("is safe for an unknown state", () => {
    expect(stageChipClass("wat")).toBe("");
  });
});
