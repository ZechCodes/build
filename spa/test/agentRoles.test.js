// Which models this device is for which roles: the shape, and what a press
// sends back.
import { describe, expect, it } from "vitest";
import {
  modelForRole,
  moved,
  rolesFilled,
  rowSummary,
  whyUnsavable,
  withCapability,
  withModel,
  withRole,
  withoutRow,
} from "../src/core/agentRoles.js";

// Zech's own example.
const declared = () => [
  { model: "claude-fable-5-1", roles: ["planner", "reviewer"], capability: "generalist" },
  { model: "claude-opus-5", roles: ["planner", "reviewer", "implementer"], capability: "scoped" },
  { provider: "codex", model: "gpt-5", roles: ["implementer", "executor"], capability: "step_by_step" },
];

describe("what fills a role", () => {
  // The order is the preference: Fable and Opus both review, Fable is above.
  it("is the first model in the list that can", () => {
    expect(modelForRole(declared(), "reviewer").model).toBe("claude-fable-5-1");
    expect(modelForRole(declared(), "implementer").model).toBe("claude-opus-5");
  });

  it("narrows on capability, and answers nothing when nobody fits", () => {
    expect(modelForRole(declared(), "implementer", "step_by_step").model).toBe("gpt-5");
    expect(modelForRole(declared(), "executor", "generalist")).toBeNull();
    expect(modelForRole([], "reviewer")).toBeNull();
  });

  it("says which roles have a model at all", () => {
    expect([...rolesFilled(declared())].sort()).toEqual([
      "executor",
      "implementer",
      "planner",
      "reviewer",
    ]);
    expect([...rolesFilled([])]).toEqual([]);
  });
});

describe("editing the list", () => {
  it("toggles a role without disturbing the other rows", () => {
    const before = declared();
    const after = withRole(before, 0, "implementer", true);
    expect(after[0].roles).toEqual(["planner", "implementer", "reviewer"]);
    expect(after[1]).toEqual(before[1]);
    // The panel's own copy is untouched, so a refused save leaves it.
    expect(before).toEqual(declared());

    expect(withRole(declared(), 0, "planner", false)[0].roles).toEqual(["reviewer"]);
  });

  it("sets a capability, adds a model and removes a row", () => {
    expect(withCapability(declared(), 2, "scoped")[2].capability).toBe("scoped");
    const added = withModel(declared(), { model: "claude-haiku-4-5" });
    expect(added).toHaveLength(4);
    expect(added[3]).toEqual({ model: "claude-haiku-4-5", roles: [], capability: "scoped" });
    expect(withoutRow(declared(), 1).map((row) => row.model)).toEqual([
      "claude-fable-5-1",
      "gpt-5",
    ]);
  });

  // Moving a row IS the edit: it changes which model fills a shared role.
  it("moves a row, and a move off either end is no move", () => {
    const after = moved(declared(), 1, -1);
    expect(after.map((row) => row.model)).toEqual([
      "claude-opus-5",
      "claude-fable-5-1",
      "gpt-5",
    ]);
    expect(modelForRole(after, "reviewer").model).toBe("claude-opus-5");
    expect(moved(declared(), 0, -1).map((row) => row.model)).toEqual(
      declared().map((row) => row.model),
    );
    expect(moved(declared(), 2, 1).map((row) => row.model)).toEqual(
      declared().map((row) => row.model),
    );
  });

  it("says why a list cannot be saved before it is sent", () => {
    expect(whyUnsavable(declared())).toBe("");
    expect(whyUnsavable([{ model: "  ", roles: [], capability: "scoped" }])).toBe(
      "Every model needs an id.",
    );
    expect(
      whyUnsavable([
        { model: "claude-opus-5", roles: [], capability: "scoped" },
        { model: "claude-opus-5", roles: [], capability: "generalist" },
      ]),
    ).toContain("in the list twice");
    // The same id on two different harnesses is two models, not a clash.
    expect(
      whyUnsavable([
        { model: "shared", roles: [], capability: "scoped" },
        { provider: "codex", model: "shared", roles: [], capability: "scoped" },
      ]),
    ).toBe("");
  });

  it("says in one line what a row is for", () => {
    expect(rowSummary(declared()[2])).toBe(
      "Implementer, Executor · give it the steps; it infers little",
    );
    expect(rowSummary({ model: "parked", roles: [], capability: "scoped" })).toContain(
      "No roles",
    );
  });
});
