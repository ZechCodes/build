import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task as wireTask } from "./trackerWireFixture.js";

const deviceId = "query-alias-device", projectId = "proj-1";
let filters, tracker, registry, engine, releases;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  filters = await import("../src/core/trackerFilters.js");
  tracker = await import("../src/core/trackerCache.js");
  const { createScopedSyncRegistry } = await import("../src/core/scopedSyncRegistry.js");
  registry = createScopedSyncRegistry(); releases = [];
});
afterEach(() => { engine?.dispose(); releases.forEach((release) => release()); registry.dispose(); });

it("shares a cold canonical query between equivalent pane mounts after the first closes", async () => {
  const { projectSyncScope } = await import("../src/core/scopedSyncRegistry.js");
  const { createScopedSyncEngine } = await import("../src/core/scopedSyncEngine.js");
  const { createScopedTaskSyncCache } = await import("../src/core/scopedTaskSyncCache.js");
  const firstParams = filters.taskListCacheParams(filters.taskListParams(projectId, { state: "open", label: "Bug" }));
  const secondParams = filters.taskListCacheParams(filters.taskListParams(projectId, { state: "open", label: "bug" }));
  const scope = projectSyncScope(projectId, { state: "open", label: "Bug" });
  const mount = (params) => {
    const { project_id: _project, ...selected } = params;
    const release = registry.register({ deviceId, projectId, scope: projectSyncScope(projectId, selected), queryParams: params });
    releases.push(release); return release;
  };
  const closeFirst = mount(firstParams);
  const active = () => registry.tracked(deviceId).some((entry) => JSON.stringify(entry.scope) === JSON.stringify(scope));
  const record = (id, revision) => {
    const { body: _body, attachments, ...task } = wireTask({ id, project_id: projectId,
      number: Number(id.slice(-1)), state: "open", labels: ["Bug"], watched: false, attachments: [] });
    return { kind: "task", key: id, revision, row: { ...task, attachment_count: attachments.length } };
  };
  let changed = false;
  const request = vi.fn(async (method) => ({ status: "ok", incarnation: "i", bridge_epoch: "e",
    ...(method === "sync.snapshot" ? { watermark: "1", cursor: "S1", records: [record("task-1", "1")] }
      : { head: changed ? "2" : "1", cursor: changed ? "C2" : "C1", changes: changed
        ? [{ ...record("task-2", "2"), seq: "2", operation: "upsert", member: true }] : [] }) }));
  const writer = createScopedTaskSyncCache({ deviceId, projectId, scope, queryParams: firstParams, bridgeIdentity: deviceId, active });
  engine = createScopedSyncEngine({ request, storeFor: () => writer, active, epoch: () => "e" });
  engine.track(scope);
  expect(await tracker.readTasksQueryCached(deviceId, projectId, secondParams)).toBeNull();
  expect(await engine.catchUp(scope)).toBe(true);
  mount(secondParams);
  expect(registry.tracked(deviceId)).toHaveLength(1);
  expect((await tracker.readTasksQueryRecord(deviceId, projectId, secondParams))?.tasks.map((task) => task.id)).toEqual(["task-1"]);
  expect(tracker.tasksQueryAddress(deviceId, projectId, firstParams)).toEqual(tracker.tasksQueryAddress(deviceId, projectId, secondParams));
  closeFirst(); changed = true;
  expect(await engine.catchUp(scope)).toBe(true);
  expect((await tracker.readTasksQueryRecord(deviceId, projectId, secondParams))?.tasks.map((task) => task.id))
    .toEqual(["task-2", "task-1"]);
  expect(request.mock.calls.filter(([method]) => method === "sync.snapshot")).toHaveLength(1);
});
