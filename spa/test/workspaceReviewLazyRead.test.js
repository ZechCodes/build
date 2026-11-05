// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const scoped = vi.hoisted(() => ({ register: vi.fn(), release: vi.fn(), wake: vi.fn(), lazy: true }));
vi.mock("../src/core/scopedSyncRuntime.js", () => ({
  lazyViewReadsEnabled: () => scoped.lazy,
  scopedSyncOwns: () => true,
  wakeScopedSync: (...args) => scoped.wake(...args),
}));
vi.mock("../src/core/scopedSyncRegistry.js", () => ({
  taskSyncScope: (taskId) => ({ kind: "task", task_id: taskId }),
  registerScopedSync: (...args) => { scoped.register(...args); return scoped.release; },
}));
vi.mock("../src/core/changeEvents.js", async (original) => ({
  ...await original(), bridgeCapabilities: () => ({ tasks: { timelinePaged: true } }),
}));

import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { rememberReviewSupport } from "../src/core/taskReviewSupport.js";
import { mountWorkspaceReviewEntry } from "../src/core/workspaceReviewEntry.js";

const scope = { deviceId: "review-open-device", projectId: "proj-1", workspaceId: "workspace-one" };
const detail = { deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: "selected-review" };
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
let entry;
beforeEach(async () => {
  scoped.register.mockClear(); scoped.release.mockClear(); scoped.wake.mockReset(); scoped.lazy = true;
  await wipeCache(); document.body.innerHTML = '<div id="review-open"></div>';
  await rememberReviewSupport(scope.deviceId, { reviews: { get: true, pullRequests: true } });
  await writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [{
    id: scope.workspaceId, workspace_id: scope.workspaceId, project_id: scope.projectId, directories: [],
    active_review: { workspace_id: scope.workspaceId, task_id: detail.sub, task_number: 1, title: "Cached selected review", status: "open", version: 1 },
  }]);
});
afterEach(() => entry?.dispose());

it("opens only selected review metadata and task scope after painting cached workspace Changes", async () => {
  const metadata = deferred(), task = deferred();
  const callRpc = vi.fn(async (method, params) => {
    if (method !== "tasks.review.get") throw new Error(`Unexpected ${method}`);
    expect(params.task_id).toBe(detail.sub);
    await metadata.promise;
    return { review: { task_id: detail.sub, workspace_id: scope.workspaceId, mode: "pull_request", version: 1, snapshots: [], bindings: [], pull_request: { status: "open" } },
      task: { id: detail.sub, title: "Fresh selected review", number: 1 } };
  });
  scoped.wake.mockImplementation(async (_deviceId, predicate) => {
    expect(predicate({ kind: "task", task_id: detail.sub })).toBe(true);
    expect(predicate({ kind: "task", task_id: "unrelated-task" })).toBe(false);
    await task.promise;
    await writeCached(detail, { task: { id: detail.sub, project_id: scope.projectId, body: "Selected body", last_reviewed_snapshot_id: "snapshot-old" },
      timeline: [], has_more: false, next_cursor: null });
    return [];
  });
  const host = document.querySelector("#review-open");
  entry = mountWorkspaceReviewEntry(host, { ...scope, callRpc });
  await vi.waitFor(() => expect(host.textContent).toContain("Cached selected review"));
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.review.get", { task_id: detail.sub }));
  expect(scoped.register).toHaveBeenCalledWith({ deviceId: scope.deviceId, projectId: scope.projectId, scope: { kind: "task", task_id: detail.sub } });
  expect(scoped.wake).toHaveBeenCalledOnce();
  metadata.resolve(); task.resolve();
  await vi.waitFor(() => expect(host.textContent).toContain("Fresh selected review"));
  await vi.waitFor(async () => expect((await readCached(detail)).value.task.last_reviewed_snapshot_id).toBe("snapshot-old"));
  expect(callRpc).toHaveBeenCalledOnce();
  entry.setVisible(false);
  expect(scoped.release).toHaveBeenCalledOnce();
  entry.dispose(); entry = null;
});
