import { expect, it } from "vitest";
import { lastReviewedSnapshot } from "../src/core/workspaceReviewState.js";

const review = { snapshots: [{ id: "snapshot-old" }, { id: "snapshot-new" }] };
it("uses complete-history review metadata when the loaded newest window omits the user's opinion", () => {
  expect(lastReviewedSnapshot(review, { task: { last_reviewed_snapshot_id: "snapshot-old" }, timeline: [], has_more: true }))
    .toEqual(review.snapshots[0]);
});
it("retains the legacy timeline derivation when the authoritative field is absent", () => {
  const timeline = [{ type: "comment", id: "tc-1", author: { kind: "user" }, opinion: { snapshot_id: "snapshot-new" } }];
  expect(lastReviewedSnapshot(review, { task: {}, timeline })).toEqual(review.snapshots[1]);
});
it("treats an explicit authoritative null as no review rather than reviving old cached history", () => {
  const timeline = [{ type: "comment", id: "tc-1", author: { kind: "user" }, opinion: { snapshot_id: "snapshot-new" } }];
  expect(lastReviewedSnapshot(review, { task: { last_reviewed_snapshot_id: null }, timeline, has_more: true })).toBeUndefined();
});
