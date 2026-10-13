import { expect, it, vi } from "vitest";

const cache = vi.hoisted(() => ({ readCached: vi.fn(), readCachedMany: vi.fn() }));
vi.mock("../src/core/localCache.js", async (original) => ({ ...await original(), ...cache }));
const { createRailWorkItem } = await import("../src/core/railWorkItem.js");

it("starts the independent legacy project row while its roster/support snapshot is pending", async () => {
  const legacy = { entity_id: "owner", agents: [{ id: "pin" }] };
  const roster = { entity_id: "owner", pinned_agent_id: "pin", agents: [{ id: "worker" }, { id: "pin" }] };
  cache.readCached.mockResolvedValue({ value: legacy });
  let finishRoster;
  cache.readCachedMany.mockImplementation(() => new Promise((resolve) => { finishRoster = resolve; }));
  const records = createRailWorkItem({ context: { kind: "project", deviceId: "device", projectId: "project" },
    railContext: {}, cacheScope: { address: (parts) => ({ deviceId: "device", ...parts }) }, alive: () => true });

  const pending = records.read("owner");
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
  const startedReads = cache.readCached.mock.calls.map(([address]) => address.kind);
  finishRoster([{ value: { projectWorkers: true } }, { value: roster }]);
  const row = await pending;

  expect(startedReads).toEqual(["row"]);
  expect(cache.readCachedMany).toHaveBeenCalledWith([
    { deviceId: "device", entityId: "", kind: "project-agents-support" },
    { deviceId: "device", entityId: "owner", kind: "project-agents" },
  ]);
  expect(row).toEqual({ ...roster, project_id: "project" });
});

it("independent project row consumers do not invalidate each other's owner read", async () => {
  const roster = { entity_id: "owner", pinned_agent_id: "pin", agents: [{ id: "worker" }, { id: "pin" }] };
  cache.readCached.mockResolvedValue({ value: { entity_id: "owner", agents: [{ id: "pin" }] } });
  cache.readCachedMany.mockResolvedValue([{ value: { projectWorkers: true } }, { value: roster }]);
  const records = createRailWorkItem({ context: { kind: "project", deviceId: "device", projectId: "project" },
    railContext: {}, cacheScope: { address: (parts) => ({ deviceId: "device", ...parts }) }, alive: () => true });

  const rows = await Promise.all([records.read("owner"), records.cachedRow("owner")]);

  expect(rows).toEqual([{ ...roster, project_id: "project" }, { ...roster, project_id: "project" }]);
});
