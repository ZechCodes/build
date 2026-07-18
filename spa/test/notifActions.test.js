import { describe, it, expect } from "vitest";
import { notifActionsFor } from "../src/core/notifActions.js";

describe("notifActionsFor", () => {
  it("gives a non-warn card a single open action carrying its primary flag", () => {
    const actions = notifActionsFor("run", { tone: "", action: "Review diff", primary: true });
    expect(actions).toEqual([{ kind: "open", label: "Review diff", primary: true }]);
  });

  it("gives a warn card open + message-agent + open-agent actions", () => {
    const actions = notifActionsFor("run", { tone: "warn", action: "View task", primary: false });
    expect(actions.map((a) => a.kind)).toEqual(["open", "message", "agent"]);
    expect(actions[0]).toEqual({ kind: "open", label: "View task", primary: false });
    expect(actions[1].label).toBe("Message agent");
    expect(actions[2].label).toBe("Open agent");
  });

  it("offers the act-now row for warn plans as well as warn runs", () => {
    expect(notifActionsFor("plan", { tone: "warn", action: "View plan" }).map((a) => a.kind)).toEqual([
      "open",
      "message",
      "agent",
    ]);
  });
});
