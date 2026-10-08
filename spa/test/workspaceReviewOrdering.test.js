// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { reviewAddress, writeReviewReply } from "../src/core/taskReviewCache.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";
import { startCacheSync, stopCacheSync, syncDevice } from "../src/core/cacheSync.js";
import opened from "../../fixtures/api/v1/tasks.review.open.json";
import pushed from "../../fixtures/api/v1/tasks.review.push.json";

const state = vi.hoisted(() => ({ context: null, watchers: [] }));
vi.mock("../src/appState.js", () => ({ App: { route: { name: "inbox" }, devices: [] } }));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => state.context,
  liveContexts: () => [],
  onDeviceStateChanged: () => () => {},
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch() }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "git", "tasks"] } }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: (watcher) => { state.watchers.push(watcher); return { dispose() {} }; },
}));

const scope = { deviceId: "review-ordering", projectId: "proj-1", workspaceId: "workspace-1", taskId: "task-1" };
const support = { get: true, pullRequests: true, push: true };
const workspace = { id: scope.workspaceId, workspace_id: scope.workspaceId, project_id: scope.projectId, name: "Work",
  directories: opened.result.review.snapshots[0].directories.filter((directory) => ["dir-api", "dir-ui"].includes(directory.id)),
  active_review: { task_id: scope.taskId, workspace_id: scope.workspaceId, version: 1, status: "open" } };
const initialReply = { ...opened.result, sync: pushed.result.sync.map((fact) => ({ ...fact, revision: 1,
  health: "pending", pending_commits: 1, working_head: "3".repeat(40) })) };
const publishedReply = pushed.examples[1].result;
const heldReview = async () => (await readCached(reviewAddress(scope))).value.review;
const header = () => document.querySelector(".workspace-review-heading").textContent;
const pushCalls = () => rpc.mock.calls.filter(([method]) => method === "tasks.review.push");
let entry, rpc, answer, pushAnswer;

beforeEach(async () => {
  await wipeCache(); await wipeUiRecords();
  document.body.innerHTML = '<div id="entry"></div>';
  state.watchers = [];
  answer = initialReply;
  pushAnswer = publishedReply;
  rpc = vi.fn(async (method) => {
    const replies = {
      "board.list": { items: [] }, "project.list": { projects: [{ project_id: scope.projectId }] },
      "workspace.list": { workspaces: [workspace] }, "tasks.columns": { columns: [{ id: "in_review" }] },
      "tasks.list": { tasks: [{ id: scope.taskId, project_id: scope.projectId,
        review_summary: { ...workspace.active_review, version: answer.review.version } }] },
      "tasks.review.get": answer, "tasks.review.push": pushAnswer,
    };
    return replies[method] || {};
  });
  state.context = { deviceId: scope.deviceId, rpc, session: {}, greeted: Promise.resolve(),
    cacheScope: {}, active: () => true, adapter: { capabilities: { reviews: support } } };
  await rememberReviewSupport(scope.deviceId, { reviews: support });
  await writeReviewReply(scope, initialReply, 0);
  startCacheSync();
  expect(await syncDevice(scope.deviceId)).toBe(true);
  entry = mountWorkspaceReviewEntry(document.querySelector("#entry"), { ...scope, callRpc: rpc });
  await vi.waitFor(() => expect(header()).toContain("Snapshot 1"));
});
afterEach(async () => {
  document.querySelector("[data-review-form-cancel]")?.click();
  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  entry?.dispose(); stopCacheSync();
});

async function publish() {
  document.querySelector('[data-review-push="dir-api"]').click();
  await vi.waitFor(() => expect(document.querySelector("[data-push-review-submit]")?.onclick).toBeTypeOf("function"));
  document.querySelector("[data-push-review-submit]").click();
  await vi.waitFor(() => expect(pushCalls()).toHaveLength(1));
}
async function taskEvent(reply) {
  answer = reply;
  const watcher = state.watchers.find(({ id }) => id === "s-inbox");
  await watcher.onChanges([{ entity_id: scope.projectId, tasks: { task_ids: [scope.taskId] } }]);
}

it("paints the authoritative cached Push snapshot and treats the matching task event as a snapshot no-op", async () => {
  const readsBeforePush = rpc.mock.calls.filter(([method]) => method === "tasks.review.get").length;
  await publish();
  await vi.waitFor(() => expect(header()).toContain("Snapshot 2"));
  expect(header()).toContain("2 snapshots");
  expect(await heldReview()).toEqual(publishedReply.review);
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.review.get")).toHaveLength(readsBeforePush);

  const snapshotIds = (await heldReview()).snapshots.map(({ id }) => id);
  await taskEvent({ review: publishedReply.review, sync: publishedReply.sync });
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.review.get")).toHaveLength(readsBeforePush + 1);
  expect((await heldReview()).snapshots.map(({ id }) => id)).toEqual(snapshotIds);
  expect((await heldReview()).snapshots).toHaveLength(2);
  expect(header()).toContain("Snapshot 2");
  expect(header()).toContain("2 snapshots");
  expect(pushCalls()).toHaveLength(1);
});

it("keeps the later task-event snapshot count when an older Push response arrives afterwards", async () => {
  const delayed = Promise.withResolvers();
  pushAnswer = delayed.promise;
  await publish();
  expect(header()).toContain("Snapshot 1");

  const previous = publishedReply.review.snapshots.at(-1);
  const latest = { ...publishedReply, review: { ...publishedReply.review, version: 5,
    pull_request: { ...publishedReply.review.pull_request, latest_published_snapshot_id: "snapshot-3" },
    snapshots: [...publishedReply.review.snapshots, { ...previous, id: "snapshot-3", number: 3 }] },
    sync: publishedReply.sync.map((fact) => ({ ...fact, revision: fact.revision + 1 })) };
  try {
    await taskEvent(latest);
    await vi.waitFor(() => expect(header()).toContain("Snapshot 3"));
    expect(header()).toContain("3 snapshots");
  } finally { delayed.resolve(publishedReply); }

  await vi.waitFor(() => expect(document.querySelector("[role=dialog]")).toBeNull());
  const review = await heldReview();
  expect(review.version).toBe(5);
  expect(review.snapshots.map(({ id }) => id)).toEqual(["snapshot-1", "snapshot-2", "snapshot-3"]);
  expect(header()).toContain("Snapshot 3");
  expect(header()).toContain("3 snapshots");
  expect(pushCalls()).toHaveLength(1);
});
