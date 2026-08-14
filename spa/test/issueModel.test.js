import { describe, it, expect } from "vitest";
import {
  STAGE_STATE_LABEL,
  stageStateToken,
  stageStateChipClass,
  plannedStageIds,
  stageApprovable,
  docLineRange,
  docMarkerGroups,
  lineageRoute,
  WORKTREE_TARGETS,
  AGENT_TARGETS,
  targetSupported,
  unsupportedTargetReason,
  defaultAssignment,
  assignmentSummary,
  implementParams,
  issueViewKey,
} from "../src/core/issueModel.js";

const stage = (overrides = {}) => ({ id: "s1", title: "Wire", state: "planned", ...overrides });

describe("stageStateToken", () => {
  it("reads the doc's approval while nothing has executed", () => {
    expect(stageStateToken(stage({ state: "planned", execution: "pending" }))).toBe("planned");
    expect(stageStateToken(stage({ approval: "approved", execution: "pending" }))).toBe("approved");
    expect(stageStateToken(stage({ state: "approved" }))).toBe("approved");
  });

  it("lets execution speak once the agent has started on the stage", () => {
    expect(stageStateToken(stage({ approval: "approved", execution: "building" }))).toBe("building");
    expect(stageStateToken(stage({ approval: "approved", execution: "built" }))).toBe("built");
    expect(stageStateToken(stage({ approval: "approved", execution: "validating" }))).toBe("validating");
    expect(stageStateToken(stage({ approval: "approved", execution: "validation_failed" }))).toBe("validation_failed");
  });

  it("calls a completed stage validated — including one whose diff is unpinned", () => {
    expect(stageStateToken(stage({ approval: "approved", execution: "complete" }))).toBe("validated");
    expect(stageStateToken(stage({ approval: "approved", execution: "legacy_unpinned" }))).toBe("validated");
  });

  it("passes the parked executions through under their own names", () => {
    for (const execution of ["blocked", "failed", "incomplete"]) {
      expect(stageStateToken(stage({ approval: "approved", execution }))).toBe(execution);
    }
  });

  it("names every token it can return", () => {
    for (const token of ["planned", "approved", "building", "built", "validating", "validated", "validation_failed", "blocked", "failed", "incomplete"]) {
      expect(STAGE_STATE_LABEL[token]).toBeTruthy();
    }
  });
});

describe("stageStateChipClass", () => {
  it("keeps a drafted stage neutral, flags the one awaiting the human, and closes the loop on done", () => {
    expect(stageStateChipClass("planned")).toBe("");
    expect(stageStateChipClass("approved")).toBe("attn");
    expect(stageStateChipClass("building")).toBe("work");
    expect(stageStateChipClass("validated")).toBe("done");
    expect(stageStateChipClass("validation_failed")).toBe("warn");
    expect(stageStateChipClass("blocked")).toBe("warn");
  });
});

describe("plannedStageIds / stageApprovable", () => {
  const stages = [
    stage({ id: "a", state: "planned" }),
    stage({ id: "b", approval: "approved", state: "approved" }),
    stage({ id: "c", state: "planned" }),
  ];

  it("names the stages an approve-all sweep would touch, in order", () => {
    expect(plannedStageIds(stages)).toEqual(["a", "c"]);
    expect(plannedStageIds([])).toEqual([]);
  });

  it("offers approve only on a stage still planned", () => {
    expect(stageApprovable(stages[0])).toBe(true);
    expect(stageApprovable(stages[1])).toBe(false);
  });
});

describe("docLineRange", () => {
  const doc = ["# Stage 1", "", "Rewrite the wire so the client reads items[].", "", "Then delete the legacy keys."].join("\n");

  it("locates a single-line passage, 1-based", () => {
    expect(docLineRange(doc, "Rewrite the wire")).toEqual({ line_start: 3, line_end: 3 });
  });

  it("spans from the first line of the passage to its last", () => {
    expect(docLineRange(doc, "Rewrite the wire so the client reads items[].\n\nThen delete the legacy keys.")).toEqual({
      line_start: 3,
      line_end: 5,
    });
  });

  it("answers null rather than guessing when the passage is not in the source", () => {
    expect(docLineRange(doc, "nothing like this")).toBeNull();
    expect(docLineRange(doc, "")).toBeNull();
    expect(docLineRange("", "anything")).toBeNull();
  });
});

describe("docMarkerGroups", () => {
  const comments = [
    { id: "message-1", body: "why?", state: "open", anchor: { heading_path: ["Stage 1", "Wire"], snippet: "items[]" } },
    { id: "message-2", body: "ok", state: "addressed", anchor: { heading_path: ["Stage 1", "Wire"], snippet: "items[]" } },
    { id: "message-3", body: "general", state: "open", anchor: null },
  ];

  it("gathers a heading's comments under one marker, keyed by the heading's slug", () => {
    const groups = docMarkerGroups(comments);
    expect(groups.map((g) => g.key)).toEqual(["wire", ""]);
    expect(groups[0].comments.map((c) => c.id)).toEqual(["message-1", "message-2"]);
    expect(groups[0].headingPath).toEqual(["Stage 1", "Wire"]);
  });

  it("counts what is still open so the marker can say whether it needs anyone", () => {
    const groups = docMarkerGroups(comments);
    expect(groups[0].open).toBe(1);
    expect(groups[0].total).toBe(2);
    expect(groups[1].open).toBe(1);
  });

  it("has nothing to mark on a doc nobody commented on", () => {
    expect(docMarkerGroups([])).toEqual([]);
    expect(docMarkerGroups(undefined)).toEqual([]);
  });
});

describe("lineageRoute", () => {
  it("routes an implementation to the branch's changes", () => {
    expect(lineageRoute({ run_id: "run-1", branch: "build/x" }, "p1")).toEqual({
      name: "task",
      projectId: "p1",
      id: "run-1",
      tab: "changes",
    });
  });

  it("has nowhere to send an implementation with no run", () => {
    expect(lineageRoute({ branch: "build/x" }, "p1")).toBeNull();
    expect(lineageRoute(null, "p1")).toBeNull();
  });
});

describe("assignment targets", () => {
  it("offers new and existing for both worktree and agent", () => {
    expect(WORKTREE_TARGETS.map((t) => t.id)).toEqual(["new", "existing"]);
    expect(AGENT_TARGETS.map((t) => t.id)).toEqual(["new", "existing"]);
  });

  it("supports only the new-worktree/new-agent handoff the bridge implements today", () => {
    expect(targetSupported(WORKTREE_TARGETS, "new")).toBe(true);
    expect(targetSupported(WORKTREE_TARGETS, "existing")).toBe(false);
    expect(targetSupported(AGENT_TARGETS, "existing")).toBe(false);
  });

  it("says why an offered target cannot be taken", () => {
    expect(unsupportedTargetReason(WORKTREE_TARGETS, "existing")).toMatch(/worktree/i);
    expect(unsupportedTargetReason(WORKTREE_TARGETS, "new")).toBeNull();
  });

  it("opens on a new worktree and a new agent carrying the issue's own model choice", () => {
    expect(defaultAssignment({ base_branch: "main", provider: "codex", model: "gpt", effort: "high" })).toEqual({
      worktree: "new",
      agent: "new",
      base: "",
      provider: "codex",
      model: "gpt",
      effort: "high",
    });
    expect(defaultAssignment(null).worktree).toBe("new");
  });

  it("summarises itself in one line for the collapsed control", () => {
    expect(assignmentSummary({ worktree: "new", agent: "new", base: "", provider: "claude" })).toBe(
      "New worktree · New agent · claude",
    );
    expect(assignmentSummary({ worktree: "new", agent: "new", base: "release", provider: "" })).toBe(
      "New worktree · New agent · release",
    );
  });
});

describe("implementParams", () => {
  const models = [{ id: "gpt", label: "GPT", supports_effort: true }];

  it("carries the issue and the assignment's overrides to implement_all", () => {
    expect(implementParams("issue-1", { worktree: "new", agent: "new", base: "release", provider: "codex", model: "gpt", effort: "high" }, { models })).toEqual({
      issue_id: "issue-1",
      base_branch: "release",
      provider: "codex",
      model: "gpt",
      effort: "high",
    });
  });

  it("omits what was left at the default", () => {
    expect(implementParams("issue-1", { worktree: "new", agent: "new", base: "  ", provider: "", model: "", effort: "" }, { models })).toEqual({
      issue_id: "issue-1",
    });
  });

  it("names the stage when one stage is being implemented", () => {
    expect(implementParams("issue-1", defaultAssignment(null), { models, stageId: "s2" })).toEqual({
      issue_id: "issue-1",
      stage_id: "s2",
    });
  });

  it("refuses to dispatch a target the bridge cannot honour", () => {
    expect(() => implementParams("issue-1", { ...defaultAssignment(null), worktree: "existing" }, { models })).toThrow(/worktree/i);
    expect(() => implementParams("issue-1", { ...defaultAssignment(null), agent: "existing" }, { models })).toThrow(/agent/i);
  });
});

describe("issueViewKey", () => {
  const issue = { state: "approved", goal: "g", active_run_id: null, implementation_lineage: [] };
  const stagesData = { stages: [{ id: "a", state: "planned" }] };

  it("holds still while nothing the surface renders has moved", () => {
    expect(issueViewKey({ issue, stagesData, selectedStageId: "a", docState: "ready", doc: "x" })).toBe(
      issueViewKey({ issue, stagesData, selectedStageId: "a", docState: "ready", doc: "x" }),
    );
  });

  it("moves when a stage, the selection, the doc, or the lineage moves", () => {
    const base = issueViewKey({ issue, stagesData, selectedStageId: "a", docState: "ready", doc: "x" });
    expect(issueViewKey({ issue, stagesData, selectedStageId: "b", docState: "ready", doc: "x" })).not.toBe(base);
    expect(issueViewKey({ issue, stagesData, selectedStageId: "a", docState: "ready", doc: "y" })).not.toBe(base);
    expect(
      issueViewKey({
        issue: { ...issue, implementation_lineage: [{ run_id: "r", state: "building" }] },
        stagesData,
        selectedStageId: "a",
        docState: "ready",
        doc: "x",
      }),
    ).not.toBe(base);
    expect(
      issueViewKey({ issue, stagesData: { stages: [{ id: "a", state: "approved" }] }, selectedStageId: "a", docState: "ready", doc: "x" }),
    ).not.toBe(base);
  });
});
