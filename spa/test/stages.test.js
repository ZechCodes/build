import { describe, it, expect } from "vitest";
import { STAGE_LABEL, stageChipClass, stageBoardHtml } from "../src/views/stages.js";

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
