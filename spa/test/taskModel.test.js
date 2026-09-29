import { describe, it, expect } from "vitest";
import {
  STAGE_STATE_LABEL,
  stageStateToken,
  stageStateChipClass,
  stageApprovable,
  stageNeighbors,
  docMarkerGroups,
  lineageRoute,
  taskViewKey,
} from "../src/core/taskModel.js";

const stage = (overrides = {}) => ({ id: "s1", title: "Wire", state: "planned", ...overrides });

describe("stageStateToken", () => {
  it("reads the doc's approval while nothing has executed", () => {
    expect(stageStateToken(stage({ state: "planned", execution: "pending" }))).toBe("planned");
    expect(stageStateToken(stage({ approval: "approved", execution: "pending" }))).toBe("approved");
    expect(stageStateToken(stage({ state: "approved" }))).toBe("approved");
  });

  it("lets execution speak once the agent has started on the stage", () => {
    expect(stageStateToken(stage({ approval: "approved", execution: "building" }))).toBe("building");
  });

  it("calls a completed stage complete — including one whose diff is unpinned", () => {
    expect(stageStateToken(stage({ approval: "approved", execution: "complete" }))).toBe("complete");
    expect(stageStateToken(stage({ approval: "approved", execution: "legacy_unpinned" }))).toBe("complete");
  });

  it("passes the parked executions through under their own names", () => {
    for (const execution of ["blocked", "failed", "incomplete"]) {
      expect(stageStateToken(stage({ approval: "approved", execution }))).toBe(execution);
    }
  });

  it("names every token it can return", () => {
    for (const token of ["planned", "approved", "building", "complete", "blocked", "failed", "incomplete"]) {
      expect(STAGE_STATE_LABEL[token]).toBeTruthy();
    }
  });

  it("has no validation vocabulary left: a stage is building, then complete", () => {
    for (const token of ["built", "validating", "validated", "validation_failed"]) {
      expect(STAGE_STATE_LABEL[token]).toBeUndefined();
    }
    expect(STAGE_STATE_LABEL.complete).toBe("COMPLETE");
  });
});

describe("stageStateChipClass", () => {
  it("keeps a drafted stage neutral, flags the one awaiting the human, and closes the loop on done", () => {
    expect(stageStateChipClass("planned")).toBe("");
    expect(stageStateChipClass("approved")).toBe("attn");
    expect(stageStateChipClass("building")).toBe("work");
    expect(stageStateChipClass("complete")).toBe("done");
    expect(stageStateChipClass("incomplete")).toBe("warn");
    expect(stageStateChipClass("blocked")).toBe("warn");
  });
});

describe("stageApprovable", () => {
  const stages = [
    stage({ id: "a", state: "planned" }),
    stage({ id: "b", approval: "approved", state: "approved" }),
    stage({ id: "c", state: "planned" }),
  ];

  it("offers approve only on a stage still planned", () => {
    expect(stageApprovable(stages[0])).toBe(true);
    expect(stageApprovable(stages[1])).toBe(false);
  });
});

describe("stageNeighbors", () => {
  const stages = [stage({ id: "a" }), stage({ id: "b" }), stage({ id: "c" })];

  it("names the stages either side of the open one, in board order", () => {
    expect(stageNeighbors(stages, "b")).toEqual({ index: 1, total: 3, previous: stages[0], next: stages[2] });
  });

  it("runs out at both ends", () => {
    expect(stageNeighbors(stages, "a").previous).toBeNull();
    expect(stageNeighbors(stages, "a").next).toBe(stages[1]);
    expect(stageNeighbors(stages, "c").next).toBeNull();
    expect(stageNeighbors([stage({ id: "only" })], "only")).toEqual({
      index: 0,
      total: 1,
      previous: null,
      next: null,
    });
  });

  it("has no neighbours to offer when nothing is open, or the open stage is gone", () => {
    expect(stageNeighbors(stages, null)).toEqual({ index: -1, total: 3, previous: null, next: null });
    expect(stageNeighbors(stages, "gone")).toEqual({ index: -1, total: 3, previous: null, next: null });
    expect(stageNeighbors([], "a")).toEqual({ index: -1, total: 0, previous: null, next: null });
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
      name: "branch",
      projectId: "p1",
      branch: "build/x",
      tab: "changes",
    });
  });

  it("has nowhere to send an implementation with no branch or no project", () => {
    expect(lineageRoute({ run_id: "run-1" }, "p1")).toBeNull();
    expect(lineageRoute({ branch: "build/x" }, null)).toBeNull();
    expect(lineageRoute(null, "p1")).toBeNull();
  });
});

describe("taskViewKey", () => {
  const task = { state: "approved", goal: "g", active_run_id: null, implementation_lineage: [] };
  const stagesData = { stages: [{ id: "a", state: "planned" }] };

  it("holds still while nothing the surface renders has moved", () => {
    expect(taskViewKey({ task, stagesData, selectedStageId: "a", docState: "ready", doc: "x" })).toBe(
      taskViewKey({ task, stagesData, selectedStageId: "a", docState: "ready", doc: "x" }),
    );
  });

  it("moves when a stage, the selection, the doc, or the lineage moves", () => {
    const base = taskViewKey({ task, stagesData, selectedStageId: "a", docState: "ready", doc: "x" });
    expect(taskViewKey({ task, stagesData, selectedStageId: "b", docState: "ready", doc: "x" })).not.toBe(base);
    expect(taskViewKey({ task, stagesData, selectedStageId: "a", docState: "ready", doc: "y" })).not.toBe(base);
    expect(
      taskViewKey({
        task: { ...task, implementation_lineage: [{ run_id: "r", state: "building" }] },
        stagesData,
        selectedStageId: "a",
        docState: "ready",
        doc: "x",
      }),
    ).not.toBe(base);
    expect(
      taskViewKey({ task, stagesData: { stages: [{ id: "a", state: "approved" }] }, selectedStageId: "a", docState: "ready", doc: "x" }),
    ).not.toBe(base);
  });

  // The rail stopped drawing the goal, so a goal the agent rewrote is not a
  // reason to replace the columns under the reviewer.
  it("holds still when only the task's goal moves", () => {
    expect(
      taskViewKey({ task: { ...task, goal: "rewritten" }, stagesData, selectedStageId: "a", docState: "ready", doc: "x" }),
    ).toBe(taskViewKey({ task, stagesData, selectedStageId: "a", docState: "ready", doc: "x" }));
  });
});
