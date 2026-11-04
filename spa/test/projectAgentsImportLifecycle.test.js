import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let imported;
beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  imported = vi.fn();
  vi.doMock("../src/core/scopedSyncRuntime.js", () => {
    imported();
    return { deviceScopedSyncOwns: () => false, wakeScopedSync: vi.fn(async () => []) };
  });
});
afterEach(() => {
  vi.doUnmock("../src/core/scopedSyncRuntime.js"); vi.unstubAllGlobals();
});

it("imports roster cache helpers without opening the scoped runtime even when Web Locks exist", async () => {
  vi.stubGlobal("navigator", { locks: { request() {} } });
  await import("../src/core/projectAgentsCache.js");
  expect(imported).not.toHaveBeenCalled();
});

it.each([
  [false, true], [false, false],
])("keeps legacy roster refresh independent of runtime with Locks=%s and device capability=%s", async (locks, device) => {
  vi.stubGlobal("navigator", locks ? { locks: { request() {} } } : {});
  const agents = await import("../src/core/projectAgentsCache.js");
  const cache = await import("../src/core/localCache.js");
  const roster = { entity_id: "legacy-owner", pinned_agent_id: "legacy-pin", agents: [{ id: "legacy-pin" }] };
  const call = vi.fn(async () => roster);
  await agents.syncProjectAgents({ deviceId: "legacy-device", entityId: "legacy-owner",
    capabilities: { agents: { projectWorkers: true }, sync: { device } }, call });
  expect(call).toHaveBeenCalledWith("agent.list", { entity_id: "legacy-owner", include_project_workers: true });
  expect((await cache.readCached(agents.projectAgentsAddress("legacy-device", "legacy-owner")))?.value).toEqual(roster);
  expect(imported).not.toHaveBeenCalled();
});
