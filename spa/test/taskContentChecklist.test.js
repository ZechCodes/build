// @vitest-environment jsdom
import { webcrypto } from "node:crypto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const deviceId = "content-checklist", projectId = "proj-1", taskId = "task-42";
const source = "- [ ] Verify", updated_at = "2026-10-29T00:00:00.000900Z";
let editor;
beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  vi.stubGlobal("crypto", webcrypto);
});
afterEach(() => { editor?.dispose(); vi.unstubAllGlobals(); });

it("retires only body observation proof on both optimistic checklist mutation and rollback", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const { createTaskChecklist } = await import("../src/core/taskChecklist.js");
  const { rememberTaskChecklistSupport } = await import("../src/core/taskChecklistSupport.js");
  const metadata = { header: { read_order: 3, updated_at }, body: { read_order: 3, updated_at } };
  await tracker.writeTaskRecord(deviceId, projectId, taskId, tracker.taskRecord({ id: taskId, project_id: projectId,
    title: "Checklist", body: source, updated_at, __task_content_cache: metadata }, []));
  await rememberTaskChecklistSupport(deviceId, { tasks: { bodyPrecondition: true } });
  let reject;
  const call = vi.fn(() => new Promise((_yes, no) => { reject = no; }));
  const refresh = vi.fn();
  editor = createTaskChecklist({ deviceId, projectId, taskId, call, refresh, onPending: vi.fn(), onFailure: vi.fn() });
  const saving = editor.press(0, true, source);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const optimistic = (await tracker.readTaskRecord(deviceId, projectId, taskId)).task;
  expect(optimistic.body).toBe("- [x] Verify");
  expect(optimistic.__task_content_cache?.header).toEqual(metadata.header);
  expect(optimistic.__task_content_cache?.body).toBeUndefined();
  reject(new Error("Refused"));
  expect(await saving).toBe(false);
  const restored = (await tracker.readTaskRecord(deviceId, projectId, taskId)).task;
  expect(restored.body).toBe(source);
  expect(restored.__task_content_cache?.header).toEqual(metadata.header);
  expect(restored.__task_content_cache?.body).toBeUndefined();
  expect(refresh).toHaveBeenCalledTimes(1);
});
