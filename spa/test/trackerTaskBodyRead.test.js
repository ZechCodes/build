import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it("checks a task read's authority inside its cache transaction, after it was queued", async () => {
  const { readTaskRecord, writeTaskRecord } = await import("../src/core/trackerCache.js");
  const newer = { task: { id: "task-347", body: "- [x] Saved", read_through: "tc-new" }, timeline: [] };
  await writeTaskRecord("device", "project", "task-347", newer);
  let current = true;
  const read = writeTaskRecord("device", "project", "task-347", {
    task: { id: "task-347", body: "- [ ] Saved" }, timeline: [],
  }, { accept: () => current });
  // A checkbox save began after the response was checked, while its cache
  // write still waited for the transaction. The old read must not land.
  current = false;
  await read;
  expect(await readTaskRecord("device", "project", "task-347")).toEqual(newer);
});
