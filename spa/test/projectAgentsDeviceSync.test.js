import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { supportsDeviceSync } from "../src/core/deviceSyncSupport.js";

const fake = vi.hoisted(() => ({ owners: new Set(), wake: vi.fn() }));
vi.mock("../src/core/scopedSyncRuntime.js", () => ({
  deviceScopedSyncOwns: (deviceId) => fake.owners.has(deviceId),
  wakeScopedSync: (...args) => fake.wake(...args),
}));
const capabilities = { sync: { device: true }, agents: { projectWorkers: true } };
const deviceId = "roster-device", entityId = "project-owner";
const roster = (name) => ({ entity_id: entityId, pinned_agent_id: "pin", agents: [{ id: "pin", name }] });
let cache, agents;

beforeEach(async () => {
  vi.resetModules();
  vi.stubGlobal("navigator", { locks: { request() {} } });
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  fake.owners.clear(); fake.wake.mockReset();
  cache = await import("../src/core/localCache.js");
  agents = await import("../src/core/projectAgentsCache.js");
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

const address = () => agents.projectAgentsAddress(deviceId, entityId);
const heldRoster = async () => (await cache.readCached(address()))?.value;

it.each([
  ["3.33.0", true, false], ["3.34.0", false, false], ["3.34.0", true, false], ["3.34.0", true, true],
])("routes roster confirmation for wire %s, capability %s and exact device ownership %s", async (version, advertised, tracked) => {
  const support = { ...capabilities, sync: { device: advertised } };
  // The runtime supplies exact scope ownership; use the same minimum-wire
  // predicate here while isolating this consumer from transport scheduling.
  if (supportsDeviceSync(support, version) && tracked) fake.owners.add(deviceId);
  fake.owners.add("another-device");
  const owned = fake.owners.has(deviceId);
  await cache.writeCached(address(), roster("Cached pin"));
  const published = roster("Confirmed pin"), afterWrite = vi.fn(), call = vi.fn(async () => published);
  fake.wake.mockImplementation(async (requestedDevice, predicate) => {
    expect(requestedDevice).toBe(deviceId);
    expect(predicate({ kind: "device" })).toBe(true);
    expect(predicate({ kind: "task", task_id: "unrelated" })).toBe(false);
    await cache.writeCached(address(), published);
    return [true];
  });
  await agents.syncProjectAgents({ deviceId, entityId, capabilities: support, call, afterWrite });
  expect(await heldRoster()).toEqual(published);
  expect(afterWrite).toHaveBeenCalledWith(published);
  if (owned) {
    expect(fake.wake).toHaveBeenCalledTimes(1); expect(call).not.toHaveBeenCalled();
  } else {
    expect(fake.wake).not.toHaveBeenCalled();
    expect(call).toHaveBeenCalledWith("agent.list", { entity_id: entityId, include_project_workers: true });
  }
});

it.each([false, true])("refuses an in-flight legacy roster after the exact device scope takes ownership (initial device capability %s)", async (advertised) => {
  const before = roster("Cached pin"); await cache.writeCached(address(), before);
  let release;
  const afterWrite = vi.fn(), call = vi.fn(() => new Promise((resolve) => { release = resolve; }));
  const pending = agents.syncProjectAgents({ deviceId, entityId, capabilities: { ...capabilities, sync: { device: advertised } }, call, afterWrite });
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  fake.owners.add(deviceId); release(roster("Stale legacy pin"));
  await pending;
  expect(await heldRoster()).toEqual(before); expect(afterWrite).not.toHaveBeenCalled();
});

it("keeps cached roster and callbacks unchanged while device confirmation is fenced", async () => {
  fake.owners.add(deviceId);
  const before = roster("Cached pin"); await cache.writeCached(address(), before);
  fake.wake.mockResolvedValue([false]);
  const afterWrite = vi.fn(), call = vi.fn(async () => roster("Legacy pin"));
  await agents.syncProjectAgents({ deviceId, entityId, capabilities, call, afterWrite });
  expect(await heldRoster()).toEqual(before); expect(call).not.toHaveBeenCalled();
  expect(afterWrite).not.toHaveBeenCalled();
});

it("does not run retired caller side effects after engine confirmation", async () => {
  fake.owners.add(deviceId); let active = true;
  fake.wake.mockImplementation(async () => {
    await cache.writeCached(address(), roster("Engine pin")); active = false; return [true];
  });
  const afterWrite = vi.fn(), call = vi.fn(async () => roster("Legacy pin"));
  await agents.syncProjectAgents({ deviceId, entityId, capabilities, active: () => active, call, afterWrite });
  expect(await heldRoster()).toEqual(roster("Engine pin")); expect(call).not.toHaveBeenCalled();
  expect(afterWrite).not.toHaveBeenCalled();
});
