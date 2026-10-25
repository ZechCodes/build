import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const address = { deviceId: "dev", entityId: "run", kind: "thread", sub: "conversation" };
let cache, createThreadHistoryGuard;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ createThreadHistoryGuard } = await import("../src/core/threadHistoryGuard.js"));
});

it("does not repaint for an initial history read with no retention change", async () => {
  await cache.writeCached(address, { thread_id: "current", items: [] });
  const changed = vi.fn();
  const history = createThreadHistoryGuard({ address, threadId: "current", onChange: changed });
  try {
    await history.ready;
    expect(changed).not.toHaveBeenCalled();
  } finally {
    history.dispose();
  }
});

it("notifies only for effective floor or generation-match changes", async () => {
  await cache.writeCached(address, { thread_id: "current", historyStartSequence: 8, items: [] });
  const changed = vi.fn();
  const history = createThreadHistoryGuard({ address, threadId: "current", onChange: changed });
  try {
    await history.ready;
    changed.mockClear();
    await history.observe({ history_start_sequence: 8 });
    expect(changed).not.toHaveBeenCalled();
    await history.observe({ history_start_sequence: 9 });
    expect(history.floor()).toBe(9);
    expect(changed).toHaveBeenCalledTimes(1);
    await cache.writeCached(address, { thread_id: "foreign", items: [] });
    await history.observe({ history_start_sequence: 9 });
    expect(history.filter([{ data: { sequence: 12 } }])).toEqual([]);
    expect(changed).toHaveBeenCalledTimes(2);
    await cache.writeCached(address, { thread_id: "current", historyStartSequence: 9, items: [] });
    await history.observe({ history_start_sequence: 9 });
    expect(history.filter([{ data: { sequence: 12 } }])).toHaveLength(1);
    expect(changed).toHaveBeenCalledTimes(3);
  } finally {
    history.dispose();
  }
});
