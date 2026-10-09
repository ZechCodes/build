// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { reviewAddress, writeReviewReply } from "../src/core/taskReviewCache.js";
import { mountTaskReviewLifecycle } from "../src/core/taskReviewLifecycle.js";
import mergeFixture from "../../fixtures/api/v1/tasks.review.merge.json";
import { deferred, whenDom } from "./reviewLifecycleHarness.js";

const scope = { deviceId: "reclaim-history-device", projectId: "proj-1", taskId: "task-1" };
const workspaceAddress = { deviceId: scope.deviceId, entityId: "", kind: "workspaces" };
const mergedHead = "3".repeat(40);
let controls;
let host;

function historyPlan(name, state) {
  const answer = structuredClone(state === "succeeded" ? mergeFixture.result : mergeFixture.examples[0].result);
  const intent = answer.merge_intents[0];
  intent.request_id = `plan-${name}`;
  intent.state = state;
  const apiSource = intent.request.sources.find((source) => source.directory_id === "dir-api");
  apiSource.head = mergedHead;
  apiSource.expected_base_head = mergedHead;
  delete intent.request.sources.find((source) => source.directory_id === "dir-ui").push;
  const actions = answer.review.actions.map((action) => ({ ...action, id: `${name}-${action.directory_id}` }));
  for (const action of actions) {
    if (action.directory_id === "dir-ui") {
      action.steps = action.steps.filter((step) => step.kind === "merge");
      continue;
    }
    action.steps.find((step) => step.kind === "merge").input_head = mergedHead;
    if (state === "succeeded") continue;
    const push = action.steps.find((step) => step.kind === "push");
    push.status = state;
    delete push.result_head;
    action.status = state;
  }
  intent.action_ids = actions.map((action) => action.id);
  return { intent, actions };
}

function historyRecord() {
  const record = structuredClone(mergeFixture.result);
  const plans = [historyPlan("A", "failed"), historyPlan("B", "interrupted"), historyPlan("C", "succeeded")];
  record.review.version = 10;
  const api = record.review.snapshots[0].directories.find((directory) => directory.id === "dir-api");
  api.head = mergedHead;
  api.base.oid = mergedHead;
  record.review.bindings.find((binding) => binding.directory_id === "dir-api").last_received_head = mergedHead;
  Object.assign(record.sync.find((row) => row.directory_id === "dir-api"), {
    comparison_base: mergedHead, working_head: mergedHead, received_head: mergedHead, snapshot_head: mergedHead,
  });
  record.review.actions = plans.flatMap((plan) => plan.actions);
  record.merge_intents = plans.map((plan) => plan.intent);
  return record;
}

function settlePublication(record, name) {
  const next = structuredClone(record);
  next.review.version += 1;
  const intent = next.merge_intents.find((row) => row.request_id === `plan-${name}`);
  const id = `${name}-published-api`;
  intent.action_ids.push(id);
  next.review.actions.push({ id, directory_id: "dir-api", snapshot_id: intent.request.snapshot_id,
    source_name: "API", source_path: "/sources/api", status: "succeeded", steps: [
      { kind: "push", status: "succeeded", remote: "origin", branch: "main", input_head: mergedHead,
        result_head: mergedHead, merge_action_id: `${name}-dir-api` },
    ] });
  return next;
}

const settledHistory = () => settlePublication(settlePublication(historyRecord(), "A"), "B");
const actionIn = (record, id) => record.review.actions.find((action) => action.id === id);
const mergeIn = (record, id) => actionIn(record, id).steps.find((step) => step.kind === "merge");
const pushIn = (record, id) => actionIn(record, id).steps.find((step) => step.kind === "push");

async function publish(record) {
  await writeReviewReply(scope, record, record.review.version);
  const cached = (await readCached(reviewAddress(scope))).value;
  controls?.update(cached.review, cached);
  return cached;
}

async function mount(record, options = {}) {
  const cached = await publish(record);
  await writeCached(workspaceAddress, [{ workspace_id: cached.review.workspace_id, locked: false }]);
  controls = mountTaskReviewLifecycle(host, { ...scope, review: cached.review, record: cached,
    snapshot: cached.review.snapshots.at(-1), support: { pullRequests: true }, repository: { mutate: vi.fn() },
    callRpc: vi.fn(), ...options });
  await controls.ready;
}

beforeEach(async () => {
  controls?.dispose();
  controls = null;
  await wipeCache();
  await wipeUiRecords();
  document.body.innerHTML = '<div id="reclaim-history"></div>';
  host = document.querySelector("#reclaim-history");
});
afterEach(() => controls?.dispose());

it("reveals Reclaim only after A and B publication holds settle while retaining B's interrupted Push history", async () => {
  const sent = deferred();
  const response = deferred();
  const callRpc = vi.fn((...args) => { sent.resolve(args); return response.promise; });
  const onReclaimed = vi.fn();
  const pending = historyRecord();
  await mount(pending, { callRpc, onReclaimed });
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
  const aSettled = settlePublication(pending, "A");
  await publish(aSettled);
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
  const bothSettled = settlePublication(aSettled, "B");
  const cached = await publish(bothSettled);
  expect(cached.merge_intents.find((intent) => intent.request_id === "plan-B").state).toBe("interrupted");
  expect(actionIn(cached, "B-dir-api").status).toBe("interrupted");
  expect(pushIn(cached, "B-dir-api").status).toBe("interrupted");
  expect(mergeIn(cached, "B-dir-ui").status).toBe("failed");
  const button = host.querySelector('[data-review-reclaim]');
  expect(button).not.toBeNull();
  expect(button.disabled).toBe(false);
  expect(host.textContent).toContain("Merged · Workspace retained");
  expect(callRpc).not.toHaveBeenCalled();

  await writeCached(workspaceAddress, [{ workspace_id: cached.review.workspace_id, locked: true }]);
  await whenDom(host, () => button.disabled);
  expect(host.querySelector('[data-review-reclaim]')).toBe(button);
  expect(button.title).toBe("Unlock the workspace to delete it");
  button.click();
  expect(callRpc).not.toHaveBeenCalled();
  await writeCached(workspaceAddress, [{ workspace_id: cached.review.workspace_id, locked: false }]);
  await whenDom(host, () => !button.disabled);
  button.click();
  button.click();
  expect(await sent.promise).toEqual(["workspace.reclaim", { workspace_id: cached.review.workspace_id }]);
  expect(callRpc).toHaveBeenCalledTimes(1);
  expect(button.disabled).toBe(true);
  response.resolve({ reclaimed: true });
  await whenDom(host, () => !button.disabled);
  expect(onReclaimed).toHaveBeenCalledTimes(1);
});

it("keeps B's hold until its own linked Push succeeds even when the same tip was published by A and C", async () => {
  const record = settledHistory();
  pushIn(record, "B-published-api").merge_action_id = "A-dir-api";
  await mount(record);
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
  const resolved = structuredClone(record);
  resolved.review.version += 1;
  pushIn(resolved, "B-published-api").merge_action_id = "B-dir-api";
  await publish(resolved);
  expect(host.querySelector('[data-review-reclaim]')?.disabled).toBe(false);
});

const unsafeHistory = [
  ["a current running intent", (record) => { record.merge_intents.find((intent) => intent.request_id === "plan-C").state = "running"; }],
  ["a running action", (record) => { actionIn(record, "C-dir-ui").status = "running"; }],
  ["a running Merge step", (record) => { mergeIn(record, "C-dir-ui").status = "running"; }],
  ["an interrupted Merge with unknown outcome", (record) => {
    actionIn(record, "B-dir-ui").status = "interrupted";
    mergeIn(record, "B-dir-ui").status = "interrupted";
  }],
  ["a pending Merge that has no definite failure", (record) => { mergeIn(record, "B-dir-ui").status = "pending"; }],
  ["a successful Merge without its result head", (record) => { delete mergeIn(record, "C-dir-ui").result_head; }],
  ["a missing historical action row", (record) => {
    record.review.actions = record.review.actions.filter((action) => action.id !== "B-dir-ui");
  }],
  ["an action without observed steps", (record) => { actionIn(record, "B-dir-ui").steps = []; }],
  ["an interrupted intent without a known settled Interrupted Push", (record) => {
    actionIn(record, "B-dir-api").status = "failed";
    pushIn(record, "B-dir-api").status = "failed";
  }],
];

it.each(unsafeHistory)("hides Reclaim after a cached update reports %s despite C's completed integration", async (_, alter) => {
  const settled = settledHistory();
  const callRpc = vi.fn();
  await mount(settled, { callRpc });
  expect(host.querySelector('[data-review-reclaim]')?.disabled).toBe(false);
  const held = structuredClone(settled);
  held.review.version += 1;
  alter(held);
  await publish(held);
  expect(host.querySelector('[data-review-reclaim]')).toBeNull();
  expect(callRpc).not.toHaveBeenCalled();
});
