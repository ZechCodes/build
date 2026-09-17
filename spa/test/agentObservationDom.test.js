// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { mountAgentObservation } from "../src/core/agentObservation.js";

const snapshot = (subject = "Test") => ({
  goal: { objective: "Ship", state: "active" },
  checklist: [{ id: "1", subject, state: "in_progress" }],
  observations: {
    goal: { support: "supported", freshness: "current", coverage: "complete" },
    checklist: { support: "supported", freshness: "current", coverage: "complete" },
  },
});

describe("the mounted goal observation", () => {
  it("updates the goal while leaving tasks to the activity viewer", () => {
    document.body.innerHTML = '<div id="host"></div>';
    const mounted = mountAgentObservation(document.querySelector("#host"));
    mounted.set(snapshot(), { generation: "one", working: true });
    mounted.set(snapshot("Verify"), { generation: "one", working: false });
    expect(document.querySelector("details")).toBe(null);
    expect(document.querySelector("#host").textContent).not.toContain("Verify");
  });

  it("replaces the goal when the provider session generation changes", () => {
    document.body.innerHTML = '<div id="host"></div>';
    const mounted = mountAgentObservation(document.querySelector("#host"));
    mounted.set(snapshot(), { generation: "one" });
    const first = document.querySelector(".agent-observation-panel");
    mounted.set({ ...snapshot("New process"), goal: { objective: "New goal", state: "active" } }, { generation: "two" });
    expect(document.querySelector(".agent-observation-panel")).not.toBe(first);
    expect(document.querySelector("#host").textContent).toContain("New goal");
  });

  it("stands up for a goal without execution surfaces and hides when nothing is displayable", () => {
    document.body.innerHTML = '<div id="host"></div>';
    const host = document.querySelector("#host");
    const mounted = mountAgentObservation(host);
    mounted.set({ goal: { objective: "Ship", state: "paused" }, observations: {} }, { generation: "one" });
    expect(host.hidden).toBe(false);
    expect(host.textContent).toContain("Goal paused");

    mounted.set({ observations: { goal: { support: "unsupported" } } }, { generation: "one" });
    expect(host.hidden).toBe(true);
    expect(host.innerHTML).toBe("");
  });
});
