import { describe, it, expect } from "vitest";
import { notifActionsFor } from "../src/core/notifActions.js";

describe("notifActionsFor", () => {
  it("gives a non-warn card a single open action carrying its primary flag", () => {
    const actions = notifActionsFor("run", { tone: "", action: "Review diff", primary: true });
    expect(actions).toEqual([{ kind: "open", label: "Review diff", primary: true }]);
  });

  it("gives a warn card open + open-agent, and nothing that talks to an agent", () => {
    const actions = notifActionsFor("run", { tone: "warn", action: "View task", primary: false });
    expect(actions.map((a) => a.kind)).toEqual(["open", "agent"]);
    expect(actions[0]).toEqual({ kind: "open", label: "View task", primary: false });
    expect(actions[1].label).toBe("Open agent");
  });

  // This list hosts no agent session, so it cannot host a conversation with one:
  // its act-now affordance takes you to the surface that does.
  it("never offers to message an agent, for runs or issues", () => {
    for (const kind of ["run", "plan"]) {
      const actions = notifActionsFor(kind, { tone: "warn", action: "View" });
      expect(actions.some((a) => a.kind === "message")).toBe(false);
    }
  });

  it("offers the act-now row for warn issues as well as warn runs", () => {
    expect(notifActionsFor("plan", { tone: "warn", action: "View issue" }).map((a) => a.kind)).toEqual([
      "open",
      "agent",
    ]);
  });
});
