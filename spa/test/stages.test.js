import { describe, it, expect } from "vitest";
import { STAGE_LABEL, stageChipClass } from "../src/views/stages.js";

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
