import { describe, expect, it } from "vitest";
import { observationPanelModel } from "../src/core/agentObservationModel.js";

const current = { support: "supported", freshness: "current", coverage: "complete" };

describe("the goal and checklist observation model", () => {
  it("keeps an idle active goal active without claiming execution", () => {
    const model = observationPanelModel({
      goal: { objective: "Ship the release", state: "active" },
      observations: { goal: current },
    }, { working: false });

    expect(model.goal).toMatchObject({ objective: "Ship the release", status: "Goal active", live: false });
  });

  it("adds execution wording only while current evidence is working", () => {
    const surfaces = {
      goal: { objective: "Ship", state: "active" },
      observations: { goal: { ...current } },
    };
    expect(observationPanelModel(surfaces, { working: true }).goal.status).toBe("Goal active · Running");
    surfaces.observations.goal.freshness = "stale";
    expect(observationPanelModel(surfaces, { working: true }).goal.status).toBe("Goal active");
  });

  it("preserves an unknown goal state instead of interpreting it", () => {
    const model = observationPanelModel({
      goal: { objective: "Ship", state: "awaiting_quota" },
      observations: { goal: current },
    });

    expect(model.goal.status).toBe("Goal state: awaiting_quota");
  });

  it("describes a complete checklist with its current step and exact progress", () => {
    const model = observationPanelModel({
      checklist: [
        { id: "1", subject: "Read", state: "completed" },
        { id: "2", subject: "Test", state: "in_progress" },
        { id: "3", subject: "Ship", state: "pending" },
      ],
      observations: { checklist: { ...current } },
    });

    expect(model.checklist).toMatchObject({ currentStep: "Test", progress: "1/3", stale: false, priorTurn: false });
  });

  it("never invents a total for partial evidence", () => {
    const model = observationPanelModel({
      checklist: [{ id: "1", subject: "Known", state: "completed" }],
      observations: { checklist: { ...current, coverage: "partial", omitted_count: 4 } },
    });

    expect(model.checklist.progress).toBe("1 known completed · 4 omitted");
    expect(model.checklist.progress).not.toContain("/");
  });

  it("names no current step after every known item completed", () => {
    const model = observationPanelModel({
      checklist: [
        { id: "1", subject: "Read", state: "completed" },
        { id: "2", subject: "Test", state: "completed" },
      ],
      observations: { checklist: { ...current } },
    });

    expect(model.checklist.currentStep).toBe("");
    expect(model.checklist.progress).toBe("2/2");
  });

  it("labels stale prior-turn context and never marks it live", () => {
    const model = observationPanelModel({
      checklist: [{ id: "1", subject: "Resume", state: "in_progress" }],
      checklist_provenance: { carried_from_prior_turn: true },
      observations: { checklist: { support: "supported", freshness: "stale", coverage: "complete" } },
    });

    expect(model.checklist).toMatchObject({ stale: true, priorTurn: true, live: false });
    expect(model.checklist.notes).toEqual(["Last known", "Prior turn"]);
  });

  it("hides unsupported or known-empty observations without saying no goal", () => {
    expect(observationPanelModel({ observations: { goal: { support: "unsupported" } } })).toEqual({
      goal: null,
      checklist: null,
    });
    expect(observationPanelModel({ checklist: [], observations: { checklist: current } }).checklist).toBe(null);
  });
});
