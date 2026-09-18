import { describe, expect, it } from "vitest";
import { createTaskCompletionTracker } from "../src/core/taskCompletionModel.js";

const agent = (checklist, over = {}) => ({
  id: "ag-1",
  surface_session_generation: "session-1",
  surfaces: {
    checklist,
    observations: { checklist: { support: "supported", freshness: "current", coverage: "complete" } },
    checklist_provenance: {
      source: "turn_plan", provider_session_generation: 4, turn_id: "turn-8",
      collection_epoch: 10, carried_from_prior_turn: false,
    },
  },
  ...over,
});

const task = (state, over = {}) => ({ id: "turn-8:0", subject: "Ship the fix", state, ...over });

describe("task completion transitions", () => {
  it("uses the first snapshot as a baseline, including already completed tasks", () => {
    const tracker = createTaskCompletionTracker();
    expect(tracker.observe(agent([task("completed")]))).toEqual([]);
    expect(tracker.observe(agent([task("completed")]))).toEqual([]);
  });

  it("announces a provider-shaped completion when its collection epoch advances", () => {
    const tracker = createTaskCompletionTracker();
    tracker.observe(agent([task("in_progress")]));
    const finished = agent([task("completed")]);
    finished.surfaces.checklist_provenance.collection_epoch = 11;
    expect(tracker.observe(finished)).toEqual([{ id: "turn-8:0", title: "Ship the fix" }]);
    expect(tracker.observe(finished)).toEqual([]);
    const regressed = agent([task("in_progress")]);
    regressed.surfaces.checklist_provenance.collection_epoch = 10;
    expect(tracker.observe(regressed)).toEqual([]);
    expect(tracker.observe(finished)).toEqual([]);
  });

  it("rejects stale, prior-turn, regressed, and replacement snapshots", () => {
    const tracker = createTaskCompletionTracker();
    tracker.observe(agent([task("in_progress")]));
    const stale = agent([task("completed")]);
    stale.surfaces.observations.checklist.freshness = "stale";
    expect(tracker.observe(stale)).toEqual([]);
    const prior = agent([task("completed")]);
    prior.surfaces.checklist_provenance.carried_from_prior_turn = true;
    expect(tracker.observe(prior)).toEqual([]);
    const nextTurn = agent([task("completed")]);
    nextTurn.surfaces.checklist_provenance.turn_id = "turn-9";
    expect(tracker.observe(nextTurn)).toEqual([]);
    const replay = agent([task("completed")]);
    replay.surfaces.checklist_provenance.collection_epoch = 9;
    expect(tracker.observe(replay)).toEqual([]);
  });

  it("resets on session generation and ignores an id reused for another subject", () => {
    const tracker = createTaskCompletionTracker();
    tracker.observe(agent([task("in_progress")]));
    expect(tracker.observe(agent([task("completed", { subject: "A different task" })]))).toEqual([]);
    expect(tracker.observe(agent([task("completed")], { surface_session_generation: "session-2" }))).toEqual([]);
  });

  it("tracks background agents independently", () => {
    const tracker = createTaskCompletionTracker();
    tracker.observe(agent([task("in_progress")]));
    tracker.observe({ ...agent([task("in_progress")]), id: "ag-2" });
    expect(tracker.observe({ ...agent([task("completed")]), id: "ag-2" })).toHaveLength(1);
    expect(tracker.observe(agent([task("in_progress")]))).toEqual([]);
  });
});
