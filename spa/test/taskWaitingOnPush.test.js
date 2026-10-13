/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, changes;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/taskWaitingOnCache.js");
  changes = await import("../src/core/changeEvents.js");
  changes.armChangeEvents({ push_events: true }, "d");
});
afterEach(() => changes.resetChangeEvents());

it("a tasks push invalidates reverse pages without a mounted task and rejects prior replies", async () => {
  for (const project of ["p", "other"]) {
    await cache.cacheTaskWaitingOnBlocks("d", project, "blocker", { blocks: [{ task_id: "off-preview", title: "Old", state: "open" }] });
  }
  const older = await cache.captureTaskWaitingOnBlocksRead("d", "p", "blocker");
  expect(changes.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "p", tasks: { key: "new" } }] }, "d")).toBe(true);
  await vi.waitFor(async () => expect(await cache.readTaskWaitingOnBlocks("d", "p", "blocker")).toBeNull());
  await cache.cacheTaskWaitingOnBlocks("d", "p", "blocker", { blocks: [{ task_id: "off-preview", title: "Old", state: "open" }] }, older);
  expect(await cache.readTaskWaitingOnBlocks("d", "p", "blocker")).toBeNull();
  expect((await cache.readTaskWaitingOnBlocks("d", "other", "blocker")).blocks).toEqual([{ task_id: "off-preview", title: "Old", state: "open" }]);
});

it("other kinds and an unarmed device cannot invalidate reverse pages", async () => {
  await cache.cacheTaskWaitingOnBlocks("d", "p", "blocker", { blocks: [{ task_id: "dependent" }] });
  changes.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "p", state: { key: "new" } }] }, "d");
  changes.disarmChangeEvents("d");
  expect(changes.dispatchChangeEvent({ type: "changes", items: [{ entity_id: "p", tasks: { key: "new" } }] }, "d")).toBe(false);
  expect((await cache.readTaskWaitingOnBlocks("d", "p", "blocker")).blocks).toEqual([{ task_id: "dependent" }]);
});
