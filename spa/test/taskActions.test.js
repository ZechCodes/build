import { describe, it, expect } from "vitest";
import { canDelete, canAbandon, mergeFailureReason } from "../src/core/taskActions.js";

describe("task removal actions match the bridge contract", () => {
  it("delete is offered only for terminal states (merged/abandoned/failed)", () => {
    for (const s of ["merged", "abandoned", "failed"]) expect(canDelete(s)).toBe(true);
    for (const s of ["planning", "building", "plan_review", "review", "blocked", "idle_unreported", "interrupted", "created"])
      expect(canDelete(s)).toBe(false);
  });

  it("abandon is offered for every live, non-deletable state", () => {
    for (const s of ["planning", "building", "plan_review", "review", "blocked", "idle_unreported", "interrupted", "created"])
      expect(canAbandon(s)).toBe(true);
    for (const s of ["merged", "abandoned", "failed"]) expect(canAbandon(s)).toBe(false);
  });

  it("delete and abandon are mutually exclusive (exactly one removal action)", () => {
    for (const s of ["planning", "building", "plan_review", "review", "blocked", "idle_unreported", "interrupted", "created", "merged", "abandoned", "failed"])
      expect(canDelete(s)).not.toBe(canAbandon(s));
  });

  it("abandon is not offered for a missing/unknown state", () => {
    expect(canAbandon("")).toBe(false);
    expect(canAbandon(undefined)).toBe(false);
  });
});

describe("merge failure reason extraction", () => {
  it("strips the merge_failed: prefix and trims", () => {
    expect(mergeFailureReason("merge_failed: conflict in a.txt, b.txt")).toBe("conflict in a.txt, b.txt");
    expect(mergeFailureReason("merge_failed:primary checkout is on \"x\"")).toBe('primary checkout is on "x"');
  });

  it("returns null for non-merge errors", () => {
    expect(mergeFailureReason("unknown task_id")).toBeNull();
    expect(mergeFailureReason(null)).toBeNull();
    expect(mergeFailureReason(undefined)).toBeNull();
  });
});
