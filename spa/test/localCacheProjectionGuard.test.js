// @vitest-environment jsdom
// Current public-path characterization for the D5 optional final-CAS plan.
// No planned source handle, private authority or immutable-read API is seeded.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task as wireTask } from "./trackerWireFixture.js";

const scope = { deviceId: "projection-guard-device", projectId: "proj-1" };
const operationScope = { accountId: "projection-account", deviceId: scope.deviceId, principalId: "projection-principal" };
const taskId = "projection-guard-task";
const WAIT = { timeout: 3000, interval: 10 };
const task = (unread_count = 3) => wireTask({ id: taskId, project_id: scope.projectId,
  watched: true, read_through: "tc-10", watch_started_after: "te-15", dismissed_through: "te-05",
  unread_count, unread_mentions: 0, unread_agent_comments: 0, attachments: [],
  updated_at: "2026-10-29T12:00:00Z" });

let cache, tracker, content, owner, operations, node;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<output id="projection-guard-count"></output>';
  node = document.querySelector("#projection-guard-count");
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  operations = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(operationScope);
  operations.activateProjection();
  owner = null;
});

afterEach(() => {
  owner?.dispose();
  operations?.deactivateProjection();
  vi.restoreAllMocks();
});

async function selectedList() {
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  // Genuine dispatch-time allocation; no known-zero coordination is fabricated.
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([task()], [], contentRead.readOrder), { contentRead })).toBe(true);
  const record = await cache.readCached(address);
  const { createTaskUnreadDisplayOwner } = await import("../src/core/taskUnreadDisplay.js");
  owner = createTaskUnreadDisplayOwner({ scope, consumer: "tasks-pane" });
  await owner.updateTasks(record.value.tasks);
  return { address, record, selected: record.value.tasks[0] };
}

function holdDisplayMerge() {
  const merge = cache.mergeCachedRecordsTogether;
  let pending = null;
  const completed = [];
  const isDisplay = (addresses) => addresses.length === 6
    && addresses[0].kind === "tracker-tasks" && addresses[1].kind === "tracker-task"
    && addresses[2].kind === "tracker-task-reads";
  vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementation((addresses, reducer, options) => {
    if (!isDisplay(addresses)) return merge(addresses, reducer, options);
    expect(pending).toBeNull();
    // Delay only scheduling. The actual reducer/records/IDB transaction and
    // real return value run unchanged when released; no verdict is mocked.
    return new Promise((resolve, reject) => {
      pending = () => merge(addresses, reducer, options).then((written) => {
        completed.push(written);
        resolve(written);
        return written;
      }, reject);
    });
  });
  return {
    captured: () => pending !== null,
    completed,
    release: async () => {
      const resume = pending;
      pending = null;
      expect(resume).toEqual(expect.any(Function));
      return resume();
    },
  };
}

function renderSelected(selected, address) {
  const count = owner.preview(selected, null);
  node.textContent = String(count);
  expect(node.textContent).toBe("3");
  // This primitive control uses an actual successful output paint and public
  // display-owner boundary; it is not full Pane/Inbox lifecycle coverage.
  owner.rendered(selected, null, count, address);
}

async function acceptChangedList(address) {
  const operationId = "projection-guard-accepted-update";
  await operations.put({ operationId, request: { method: "tasks.update", params: { task_id: taskId } } });
  await operations.accept(operationId, { ack: {}, revision: 1,
    overlay: [{ address, kind: "merge", value: { tasks: [task(5)] } }] });
  expect(await operations.get(operationId)).toMatchObject({ revision: 1, state: "accepted", overlayActive: true });
}

describe("D5 current public-path final projection guard", () => {
  it("refuses a delayed selected display after an accepted layer changes its actual source at the same canonical write", async () => {
    const { address, record, selected } = await selectedList();
    const note = (await import("../src/core/taskReadOrder.js")).taskWatchIntentAddress(scope, taskId);
    const beforeNote = await cache.readCanonicalCached(note);
    expect(beforeNote?.value?.task_unread_display).toBeUndefined();
    const held = holdDisplayMerge();
    try {
      renderSelected(selected, address);
      await vi.waitFor(() => expect(held.captured()).toBe(true), WAIT);
      await acceptChangedList(address);
      const projected = await cache.readCached(address);
      const canonical = await cache.readCanonicalCached(address);
      expect(projected.accepted).toBe(true);
      expect(cache.recordWriteOf(projected)).toBe(cache.recordWriteOf(record));
      expect(cache.recordWriteOf(canonical)).toBe(cache.recordWriteOf(record));
      expect(projected.value.tasks[0].unread_count).toBe(5);
      expect(canonical.value.tasks[0].unread_count).toBe(3);
      expect(await cache.readCanonicalCached(note)).toEqual(beforeNote);
      expect(node.textContent).toBe("3");
      await held.release();
      // Current canonical-only CAS must not stamp the old selection onto a
      // shared note after its effective projected inputs changed.
      expect(await cache.readCanonicalCached(note)).toEqual(beforeNote);
      expect(held.completed).toEqual([false]);
    } finally {
      owner.dispose();
      if (held.captured()) await held.release();
    }
  });

  it("commits the actual selected display when canonical and projected inputs remain unchanged", async () => {
    const { address, selected } = await selectedList();
    const note = (await import("../src/core/taskReadOrder.js")).taskWatchIntentAddress(scope, taskId);
    const beforeNote = await cache.readCanonicalCached(note);
    const heard = [];
    const stop = cache.subscribeCache(note, (changed) => heard.push(changed));
    const held = holdDisplayMerge();
    try {
      renderSelected(selected, address);
      await vi.waitFor(() => expect(held.captured()).toBe(true), WAIT);
      await held.release();
      const after = await cache.readCanonicalCached(note);
      expect(after.value).toMatchObject({ ...beforeNote?.value,
        task_unread_display: { "tasks-pane": { total: 3, taskId, deviceId: scope.deviceId, projectId: scope.projectId } } });
      expect(held.completed).toEqual([true]);
      expect(heard).toEqual([note]);
      expect(node.textContent).toBe("3");
    } finally {
      stop();
      owner.dispose();
      if (held.captured()) await held.release();
    }
  });
});
