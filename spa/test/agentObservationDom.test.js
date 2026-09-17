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

describe("the mounted goal and checklist observation", () => {
  it("keeps the checklist open through same-session snapshot replacement", () => {
    document.body.innerHTML = '<div id="host"></div>';
    const mounted = mountAgentObservation(document.querySelector("#host"));
    mounted.set(snapshot(), { generation: "one", working: true });
    const fold = document.querySelector("details");
    fold.open = true;

    mounted.set(snapshot("Verify"), { generation: "one", working: false });

    expect(document.querySelector("details")).toBe(fold);
    expect(fold.open).toBe(true);
    expect(fold.textContent).toContain("Verify");
  });

  it("resets disclosure state when the provider session generation changes", () => {
    document.body.innerHTML = '<div id="host"></div>';
    const mounted = mountAgentObservation(document.querySelector("#host"));
    mounted.set(snapshot(), { generation: "one" });
    document.querySelector("details").open = true;

    mounted.set(snapshot("New process"), { generation: "two" });

    expect(document.querySelector("details").open).toBe(false);
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
