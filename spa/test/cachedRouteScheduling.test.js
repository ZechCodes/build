import { expect, it, vi } from "vitest";

const cache = vi.hoisted(() => ({
  cachedAddresses: vi.fn(), readCached: vi.fn(), readCachedMany: vi.fn(),
}));
vi.mock("../src/core/localCache.js", async (original) => ({
  ...await original(), ...cache,
}));

const { cachedRouteEntry } = await import("../src/core/cachedRows.js");

it("starts independent route lists while the row transaction is still pending", async () => {
  const row = { kind: "branch", project_id: "p1", branch: "build/login", entity_id: "run-1" };
  const address = { deviceId: "device", entityId: "run-1", kind: "row" };
  cache.cachedAddresses.mockResolvedValue([address]);
  cache.readCached.mockResolvedValue({ value: [] });
  let finishRows;
  cache.readCachedMany.mockImplementation(() => new Promise((resolve) => { finishRows = resolve; }));

  const pending = cachedRouteEntry("device", { name: "branch", projectId: "p1", branch: "build/login" });
  // Only drain microtasks: no wall-clock budget and no completed row read.
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
  const startedLists = cache.readCached.mock.calls.map(([held]) => held.kind);
  finishRows([{ value: row }]);
  const entry = await pending;

  expect(startedLists).toEqual(expect.arrayContaining(["projects", "workspaces"]));
  expect(cache.readCachedMany).toHaveBeenCalledWith([address]);
  expect(entry.entityId).toBe("run-1");
});
