import { describe, it, expect } from "vitest";
import {
  STAGE_LABEL,
  stageChipClass,
  stageBoardHtml,
  runAllControlKind,
  joinRunStages,
  runStagesFallback,
  stageGateReason,
} from "../src/views/stages.js";

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

describe("joinRunStages", () => {
  const planStages = [
    { id: "first", title: "First", summary: "the first", state: "approved", open_comments: 2 },
    { id: "second", title: "Second", summary: "the second", state: "planned", open_comments: 0 },
  ];

  it("uses the plan doc sub-state when the run has not dispatched the stage yet", () => {
    const joined = joinRunStages(planStages, []);
    expect(joined.map((s) => s.state)).toEqual(["approved", "planned"]);
    expect(joined[0].doc_state).toBe("approved");
    expect(joined[0].open_comments).toBe(2);
    expect(joined[0].title).toBe("First");
    expect(joined[0].validation).toBeNull();
  });

  it("lets the run's progress state (and validation) win once the stage is dispatched", () => {
    const runStages = [
      { id: "first", state: "validated_passed", start_sha: "abc", validation: { passed: true, findings: "ok", notes_for_next_stage: "watch the rename" } },
      { id: "second", state: "building", start_sha: "def", validation: null },
    ];
    const joined = joinRunStages(planStages, runStages);
    expect(joined.map((s) => s.state)).toEqual(["validated_passed", "building"]);
    // The plan's gate (doc_state) is preserved distinctly from the effective state.
    expect(joined[0].doc_state).toBe("approved");
    expect(joined[1].doc_state).toBe("planned");
    expect(joined[0].validation.notes_for_next_stage).toBe("watch the rename");
    expect(joined[0].start_sha).toBe("abc");
  });

  it("preserves plan order and is safe for empty inputs", () => {
    expect(joinRunStages([], [])).toEqual([]);
    expect(joinRunStages(null, null)).toEqual([]);
    expect(joinRunStages(planStages, null).map((s) => s.id)).toEqual(["first", "second"]);
  });
});

describe("runStagesFallback", () => {
  const runStages = [
    { id: "first", state: "validated_passed", start_sha: "abc", validation: { passed: true, findings: "ok", notes_for_next_stage: "" } },
    { id: "second", state: "validated_passed", start_sha: "def", validation: { passed: true, findings: "done", notes_for_next_stage: "" } },
  ];

  it("builds a board shape from the run's own progress with placeholder titles", () => {
    const stages = runStagesFallback(runStages);
    expect(stages.map((s) => s.id)).toEqual(["first", "second"]);
    expect(stages.map((s) => s.title)).toEqual(["Stage 1", "Stage 2"]);
    expect(stages.map((s) => s.state)).toEqual(["validated_passed", "validated_passed"]);
    expect(stages[0].doc_state).toBe("approved"); // a stage only ran once approved
    expect(stages[0].validation.findings).toBe("ok");
    expect(stages[0].start_sha).toBe("abc");
    expect(stages[0].open_comments).toBe(0);
  });

  it("is safe for empty/absent progress", () => {
    expect(runStagesFallback([])).toEqual([]);
    expect(runStagesFallback(null)).toEqual([]);
  });

  it("carries a null validation through when a stage has none", () => {
    expect(runStagesFallback([{ id: "a", state: "building" }])[0].validation).toBeNull();
  });
});

describe("stageBoardHtml — deleted plan fallback", () => {
  const stages = runStagesFallback([
    { id: "a", state: "validated_passed", start_sha: "x", validation: { passed: true, findings: "verified", notes_for_next_stage: "" } },
  ]);

  it("shows a note that the plan was deleted when planDeleted is set", () => {
    const html = stageBoardHtml({ state: "merged" }, { stages, auto_advance: false, planDeleted: true });
    expect(html).toContain("The plan for this run was deleted");
    expect(html).toContain('id="stagelist"'); // still a real board the poll can find
  });

  it("shows no such note under normal (plan present) rendering", () => {
    const html = stageBoardHtml({ state: "stage_gate" }, { stages, auto_advance: false });
    expect(html).not.toContain("was deleted");
  });
});

describe("stageBoardHtml", () => {
  const validatedStages = [
    {
      id: "first",
      title: "First",
      summary: "",
      state: "validated_passed",
      doc_state: "approved",
      open_comments: 0,
      validation: { passed: true, findings: "- schema matches the doc", notes_for_next_stage: "watch the rename" },
    },
    {
      id: "second",
      title: "Second",
      summary: "",
      state: "validated_passed",
      doc_state: "approved",
      open_comments: 0,
      validation: { passed: true, findings: "final stage verified end to end", notes_for_next_stage: "" },
    },
  ];

  it("renders the list container with id=stagelist so the poll freeze check can find it", () => {
    const html = stageBoardHtml({ state: "stage_gate" }, { stages: validatedStages, auto_advance: false });
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
    const html = stageBoardHtml({ state: "stage_gate" }, { stages: validatedStages, auto_advance: false });
    expect(html).not.toContain("stage-validation");
  });

  it("surfaces a failed stage's findings inline in its row", () => {
    const stages = [{ id: "a", title: "A", summary: "", state: "validated_failed", doc_state: "approved", open_comments: 0, validation: { passed: false, findings: "the migration is missing", notes_for_next_stage: "" } }];
    const html = stageBoardHtml({ state: "building" }, { stages, auto_advance: false });
    expect(html).toContain("stage-validation fail");
    expect(html).toContain("the migration is missing");
  });

  it("mounts a #runall host div (not a passive checkbox) for the run-all control", () => {
    const html = stageBoardHtml({ state: "stage_gate" }, { stages: validatedStages, auto_advance: false });
    expect(html).toContain('id="runall"');
    expect(html).toContain('class="runall"');
    // the old passive checkbox is gone — the control is mounted by the wiring
    expect(html).not.toContain('type="checkbox" id="runall"');
  });

  it("mounts a #stageaction host for the single gate action", () => {
    const html = stageBoardHtml({ state: "stage_gate" }, { stages: validatedStages, auto_advance: false });
    expect(html).toContain('id="stageaction"');
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

  it("shows the run control on a fresh all-planned board at the gate", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: planned, taskState: "stage_gate" })).toBe("run");
  });

  it("shows the run control when some stages are already approved", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: someApproved, taskState: "stage_gate" })).toBe("run");
  });

  it("still shows the run control mid-run (unfinished stages remain)", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: midRun, taskState: "building" })).toBe("run");
  });

  it("shows the stop control whenever auto-advance is already on", () => {
    expect(runAllControlKind({ autoAdvance: true, stages: planned, taskState: "building" })).toBe("stop");
    expect(runAllControlKind({ autoAdvance: true, stages: allDone, taskState: "review" })).toBe("stop");
  });

  it("shows nothing once every stage has validated (nothing left to run)", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: allDone, taskState: "review" })).toBe("none");
  });

  it("shows nothing on a terminal run (merged/abandoned/archived)", () => {
    for (const s of ["merged", "abandoned", "archived"]) {
      expect(runAllControlKind({ autoAdvance: false, stages: planned, taskState: s })).toBe("none");
    }
  });

  it("shows nothing when there are no stages", () => {
    expect(runAllControlKind({ autoAdvance: false, stages: [], taskState: "stage_gate" })).toBe("none");
  });

  it("shows nothing when the earliest unfinished stage failed validation (needs a fix, not run-all)", () => {
    // Run-all would arm auto-advance, but the bridge kickstart never dispatches a
    // validated_failed stage — a silent no-op. The failed stage's own fix bar is
    // the path forward.
    const failedThenPlanned = [
      { id: "a", state: "validated_failed" },
      { id: "b", state: "planned" },
    ];
    expect(runAllControlKind({ autoAdvance: false, stages: failedThenPlanned, taskState: "building" })).toBe("none");
  });

  it("still shows the run control when a passed stage precedes a runnable one, even if a later stage failed", () => {
    const runnableBeforeFailure = [
      { id: "a", state: "validated_passed" },
      { id: "b", state: "planned" },
      { id: "c", state: "validated_failed" },
    ];
    expect(runAllControlKind({ autoAdvance: false, stages: runnableBeforeFailure, taskState: "building" })).toBe("run");
  });
});

describe("stageGateReason", () => {
  const stages = [
    { id: "a", title: "First", state: "validated_passed", doc_state: "approved" },
    { id: "b", title: "Second", state: "approved", doc_state: "approved" },
    { id: "c", title: "Third", state: "planned", doc_state: "planned" },
  ];

  it("is ready (null) for an approved stage at the gate with all priors passed", () => {
    expect(stageGateReason(stages, 1, "stage_gate")).toBeNull();
  });

  it("points the user at the plan when the stage doc is not approved yet", () => {
    expect(stageGateReason(stages, 2, "stage_gate")).toMatch(/approve/i);
    expect(stageGateReason(stages, 2, "stage_gate")).toMatch(/plan/i);
  });

  it("explains a prior stage still validating", () => {
    const s = [
      { id: "a", title: "First", state: "building", doc_state: "approved" },
      { id: "b", title: "Second", state: "approved", doc_state: "approved" },
    ];
    expect(stageGateReason(s, 1, "stage_gate")).toMatch(/waiting on validation of “First”/i);
  });

  it("explains that a stage is already running when the run is not at the gate", () => {
    expect(stageGateReason(stages, 1, "building")).toMatch(/already running/i);
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
