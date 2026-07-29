// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { taskRemovalAction } from "../src/views/task.js";

describe("task conversation lifecycle action", () => {
  it("offers no removal action while an adopted task is live", () => {
    expect(taskRemovalAction({ state: "building", adopted: true })).toBeNull();
  });

  it("offers no removal action while an ordinary task is live", () => {
    expect(taskRemovalAction({ state: "building", adopted: false })).toBeNull();
  });

  it("keeps Delete available after the task reaches a terminal state", () => {
    expect(taskRemovalAction({ state: "merged", adopted: true })).toEqual({
      id: "deleteTask",
      label: "Delete",
      busyLabel: "deleting…",
    });
  });
});
