// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { expect, it } from "vitest";
import { writeReviewReply, reviewAddress } from "../src/core/taskReviewCache.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { boundSourceFacts } from "../src/core/workspaceReviewState.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../fixtures/api/v1/tasks.review.push.json";

// #453 repro: a retarget can publish a new snapshot beside the stored old
// observation, whose count was taken for an older head.
it.each([
  { rewritten: false, expected: "Changed since your last review" },
  { rewritten: true, expected: "History rewritten since your last review" },
])("new snapshot with stale count 0 and rewritten=$rewritten", async ({ rewritten, expected }) => {
  await wipeCache();
  const scope = { deviceId: "repro", projectId: "proj-1", taskId: "task-1" };
  const baseline = opened.result.review.snapshots[0];
  const binding = opened.result.review.bindings[0];
  const oldHead = baseline.directories.find((directory) => directory.id === binding.directory_id).head;
  const observation = { ...pushed.result.sync[0], directory_id: binding.directory_id, revision: 4,
    reviewed_snapshot_id: baseline.id, received_head: oldHead, snapshot_head: oldHead, commits_since_review: 0 };
  await writeReviewReply(scope, { ...opened.result, sync: [observation] }, 1);
  const next = { ...baseline, id: "new-snapshot", number: 2,
    directories: baseline.directories.map((directory) => ({ ...directory, head: "4".repeat(40) })),
    publication: { reason: "base_changed", directories: [{ directory_id: binding.directory_id, rewritten }] } };
  // Retarget can publish the currently received tip before observations are reconciled.
  await writeReviewReply(scope, { review: { ...opened.result.review, version: 2, snapshots: [baseline, next] }, sync: [observation] }, 2);
  const held = (await readCached(reviewAddress(scope))).value;
  expect(held.sync[0].commits_since_review).toBe(0);
  const facts = boundSourceFacts({ held, reviewedSnapshot: baseline, sources: [] });
  const fact = facts.find((entry) => entry.binding.directory_id === binding.directory_id);
  expect(fact.sinceReview).toBe(expected);
});
