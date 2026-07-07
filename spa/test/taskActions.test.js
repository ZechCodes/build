import { describe, it, expect } from "vitest";
import { canDelete, canAbandon, mergeFailureReason, bannerText } from "../src/core/taskActions.js";

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

describe("error banner precedence", () => {
  it("a held local error wins over the polled last_error", () => {
    // The whole point: a failed delete/abandon (localError) must not be wiped by
    // the next poll, which supplies the task's last_error (often null).
    expect(bannerText("error: task store io", null)).toBe("error: task store io");
    expect(bannerText("error: task store io", "merge_failed: x")).toBe("error: task store io");
  });

  it("falls back to the polled last_error when there is no local error", () => {
    expect(bannerText(null, "merge_failed: conflict")).toBe("merge_failed: conflict");
  });

  it("is empty (banner hidden) when neither is set", () => {
    expect(bannerText(null, null)).toBe("");
    expect(bannerText(null, undefined)).toBe("");
  });
});
