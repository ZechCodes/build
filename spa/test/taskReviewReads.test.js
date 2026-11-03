import { beforeEach, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";
import { rememberReviewSupport, readReviewSupport } from "../src/core/taskReviewSupport.js";
import { createReviewCommitsReader, reviewCommitsAddress, reviewDiffAddress, reviewDiffParams, refreshCachedReviewReads } from "../src/core/taskReviewReads.js";

vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => null }));
const scope = { deviceId: "device", projectId: "project", taskId: "task", snapshot: { id: "revision" }, directory: { id: "repo" } };
const commit = "a".repeat(40);
const capabilities = ["tasks.review.diff", "tasks.review.commits", "tasks.review.commit", "tasks.review.since", "tasks.review.anchorCommit"];
beforeEach(async () => { await wipeCache(); });

it("caches each independently advertised diff capability", async () => {
  const caps = capabilitiesOf({ api_version: "3.28.0", capabilities });
  expect(caps.reviews).toMatchObject({ commits: true, commit: true, since: true, anchorCommit: true });
  await rememberReviewSupport(scope.deviceId, caps);
  expect(await readReviewSupport(scope.deviceId)).toMatchObject({ commits: true, commit: true, since: true, anchorCommit: true });
  await rememberReviewSupport(scope.deviceId, { reviews: { diff: true } });
  expect(await readReviewSupport(scope.deviceId)).toMatchObject({ commits: false, commit: false, since: false, anchorCommit: false });
});

it("preserves range addresses and isolates commit/since lists and patches", () => {
  expect(reviewDiffAddress(scope)).toEqual({ deviceId: "device", entityId: "project", kind: "task-review-changes", sub: JSON.stringify(["task", "revision", "repo"]) });
  expect(reviewDiffAddress(scope, {}, "a.js").sub).toBe(JSON.stringify(["task", "revision", "repo", "a.js", "old..new"]));
  const first = reviewDiffAddress(scope, { mode: "commit", commit }, "a.js");
  expect(first.kind).toBe("task-review-commit-patch");
  expect(first).not.toEqual(reviewDiffAddress(scope, { mode: "commit", commit: "b".repeat(40) }, "a.js"));
  expect(first).not.toEqual(reviewDiffAddress(scope, { mode: "since", from_snapshot_id: "prior" }, "a.js"));
  expect(reviewDiffParams(scope, { mode: "commit", commit })).toEqual({ task_id: "task", snapshot_id: "revision", directory_id: "repo", mode: "commit", commit });
});

it("paints held commit lists before an unavailable request and keeps pagination distinct", async () => {
  await rememberReviewSupport(scope.deviceId, capabilitiesOf({ api_version: "3.28.0", capabilities }));
  const held = { commits: [{ sha: commit, subject: "Saved work" }], next_cursor: "next", commits_truncated: true };
  await writeCached(reviewCommitsAddress(scope), held);
  const onChange = vi.fn();
  const callRpc = vi.fn().mockRejectedValue(new Error("Source unavailable"));
  const reader = createReviewCommitsReader({ ...scope, callRpc, onChange });
  await reader.ready;
  expect(onChange.mock.calls[0][0]).toEqual(held);
  await reader.refresh();
  expect(onChange.mock.calls.at(-1)[0]).toMatchObject({ ...held, read_error: "Source unavailable" });
  callRpc.mockResolvedValue({ commits: [{ sha: "b".repeat(40) }], commits_truncated: false });
  await reader.more();
  expect(callRpc).toHaveBeenLastCalledWith("tasks.review.diff", expect.objectContaining({ mode: "commits", cursor: "next" }));
  expect((await readCached(reviewCommitsAddress(scope))).value.commits).toHaveLength(2);
  reader.dispose();
});

it("does not probe commits when cached support is absent", async () => {
  const callRpc = vi.fn();
  const reader = createReviewCommitsReader({ ...scope, callRpc });
  await reader.ready;
  await reader.refresh();
  expect(callRpc).not.toHaveBeenCalled();
  reader.dispose();
});

it("refreshes failed comparisons and latest commit lists on reconnect without cross-task reads", async () => {
  const support = { diff: true, commits: true, commit: true, since: true };
  await writeCached(reviewCommitsAddress(scope), { commits: [] });
  await writeCached(reviewDiffAddress(scope, { mode: "commit", commit }), { files: [], read_error: "Source unavailable" });
  await writeCached(reviewDiffAddress(scope, { mode: "since", from_snapshot_id: "prior" }), { files: [], read_error: "Source unavailable" });
  await writeCached(reviewCommitsAddress({ ...scope, taskId: "other" }), { commits: [] });
  const callRpc = vi.fn(async (_method, params) => params.mode === "commits" ? { commits: [{ sha: commit }] } : { files: [{ path: "a.js" }] });
  await refreshCachedReviewReads({ ...scope, callRpc, support, latestSnapshotIds: new Map([[scope.taskId, scope.snapshot.id]]) });
  expect(callRpc.mock.calls.map(([, params]) => params.mode).sort()).toEqual(["commit", "commits", "since"]);
  expect(callRpc.mock.calls.every(([, params]) => params.task_id === "task")).toBe(true);
  expect((await readCached(reviewDiffAddress(scope, { mode: "commit", commit }))).value.files[0].path).toBe("a.js");
});


it("does not refresh comparison features without the base diff capability", async () => {
  await writeCached(reviewCommitsAddress(scope), { commits: [] });
  const callRpc = vi.fn();
  await refreshCachedReviewReads({ ...scope, callRpc, support: { commits: true } });
  expect(callRpc).not.toHaveBeenCalled();
});


it("leaves successful immutable comparisons and older commit lists untouched on reconnect", async () => {
  const support = { diff: true, commits: true, commit: true, since: true };
  await writeCached(reviewCommitsAddress(scope), { commits: [{ oid: commit }] });
  await writeCached(reviewCommitsAddress({ ...scope, snapshot: { id: "latest" } }), { commits: [] });
  await writeCached(reviewDiffAddress(scope, { mode: "commit", commit }), { files: [] });
  await writeCached(reviewDiffAddress(scope, { mode: "since", from_snapshot_id: "prior" }), { files: [] });
  const callRpc = vi.fn(async () => ({ commits: [{ oid: commit }] }));
  await refreshCachedReviewReads({ ...scope, callRpc, support, latestSnapshotIds: new Map([[scope.taskId, "latest"]]) });
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.review.diff", expect.objectContaining({ mode: "commits", snapshot_id: "latest" }));
});

it("mounts a cached commit rail without refreshing and retains loaded pages on explicit first-page refresh", async () => {
  await rememberReviewSupport(scope.deviceId, capabilitiesOf({ api_version: "3.28.0", capabilities }));
  const held = { commits: [{ oid: commit }, { oid: "b".repeat(40) }], next_cursor: "third", commits_truncated: true };
  await writeCached(reviewCommitsAddress(scope), held);
  const callRpc = vi.fn(async () => ({ commits: [{ oid: commit }], next_cursor: "second", commits_truncated: true }));
  const reader = createReviewCommitsReader({ ...scope, callRpc });
  await reader.ready;
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(callRpc).not.toHaveBeenCalled();
  await reader.refresh();
  expect((await readCached(reviewCommitsAddress(scope))).value).toMatchObject(held);
  reader.dispose();
});

it("keeps a fully loaded commit rail complete when its first page is refreshed", async () => {
  await rememberReviewSupport(scope.deviceId, capabilitiesOf({ api_version: "3.28.0", capabilities }));
  const held = { commits: [{ oid: commit }, { oid: "b".repeat(40) }], commits_truncated: false };
  await writeCached(reviewCommitsAddress(scope), held);
  const callRpc = vi.fn(async () => ({ commits: [{ oid: commit }], next_cursor: "second", commits_truncated: true }));
  const reader = createReviewCommitsReader({ ...scope, callRpc });
  await reader.ready;
  await reader.refresh();
  const value = (await readCached(reviewCommitsAddress(scope))).value;
  expect(value.commits).toEqual(held.commits);
  expect(value.commits_truncated).toBe(false);
  expect(value.next_cursor).toBeUndefined();
  reader.dispose();
});
