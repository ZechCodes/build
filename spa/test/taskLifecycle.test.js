// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { taskRemovalAction } from "../src/views/task.js";

describe("task conversation lifecycle action", () => {
  it("offers a quiet abandon action instead of Release for adopted tasks", () => {
    const action = taskRemovalAction({ state: "building", adopted: true });

    expect(action).toEqual({
      id: "abandonTask",
      label: "Abandon & delete",
      busyLabel: "abandoning…",
    });
    expect(JSON.stringify(action)).not.toContain("Release");
  });

  it("keeps the shorter abandon label for ordinary tasks", () => {
    expect(taskRemovalAction({ state: "building", adopted: false })?.label).toBe("Abandon");
  });

  it("returns no action when the task cannot be abandoned", () => {
    expect(taskRemovalAction({ state: "merged", adopted: true })).toBeNull();
  });
});
