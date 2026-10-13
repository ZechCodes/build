import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { projectAgentsAddress, readProjectAgents, rememberProjectAgentsSupport, syncProjectAgents } from "../src/core/projectAgentsCache.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const pin = { id: "pin", project_role: "coordinator" };
const worker = { id: "worker", project_role: "worker", conversation_id: "worker-chat" };
const roster = { entity_id: "owner", pinned_agent_id: "pin", agents: [worker, pin] };
const capabilities = { agents: { projectWorkers: true }, tasks: { assignProjectAgent: true } };
beforeEach(async () => { await wipeCache(); });

describe("complete cached project agents", () => {
  it("paints a complete roster only with cached support and never picks a pin by position", async () => {
    await writeCached(projectAgentsAddress("device", "owner"), roster);
    expect(await readProjectAgents("device", "owner")).toBeNull();
    await rememberProjectAgentsSupport("device", capabilities);
    expect(await readProjectAgents("device", "owner")).toEqual(roster);
    await rememberProjectAgentsSupport("device", {});
    expect(await readProjectAgents("device", "owner")).toBeNull();
  });
  it("does not send worker params to older bridges", async () => {
    const call = vi.fn(async () => roster);
    await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities: {}, call });
    expect(call).not.toHaveBeenCalled();
  });
  it("writes exact owners only, and keeps colliding device owners separate", async () => {
    await rememberProjectAgentsSupport("device", capabilities);
    const call = vi.fn(async () => roster);
    await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities, call });
    expect(call).toHaveBeenCalledWith("agent.list", { entity_id: "owner", include_project_workers: true });
    expect(await readProjectAgents("device", "owner")).toEqual(roster);
    expect(await readProjectAgents("other", "owner")).toBeNull();
    await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities, call: async () => ({ ...roster, entity_id: "foreign" }) });
    expect((await readCached(projectAgentsAddress("device", "owner"))).value).toEqual(roster);
  });
  it("an older list cannot revive a removed worker or a retired reader", async () => {
    let finish;
    const pending = syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities,
      call: () => new Promise((resolve) => { finish = resolve; }) });
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    await writeCached(projectAgentsAddress("device", "owner"), { ...roster, agents: [pin] });
    finish(roster);
    await pending;
    expect((await readCached(projectAgentsAddress("device", "owner"))).value.agents).toEqual([pin]);
    await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities, call: async () => roster, active: () => false });
    expect((await readCached(projectAgentsAddress("device", "owner"))).value.agents).toEqual([pin]);
  });
  it("refuses a roster digest from a retired transcript generation", async () => {
    await writeCached({ deviceId: "device", entityId: "owner", kind: "thread", sub: "worker-chat" },
      { thread_id: "fresh-thread", thread_generation_revision: 2, retired_thread_ids: ["old-thread"] });
    await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities, call: async () => ({ ...roster,
      agents: [{ ...worker, thread_id: "old-thread", thread_generation_revision: 1 }, pin] }) });
    expect((await readCached(projectAgentsAddress("device", "owner")))?.value).toBeFalsy();
  });
});

it("feed projections retain exact cached worker models and never duplicate the legacy pin", async () => {
  const { agentModelsFromFeed } = await import("../src/core/agentModelBadge.js");
  const workerWithModel = { ...worker, model: "gpt-6.1-sol" };
  const feed = { projects: [{ projectKey: "device/project", projectRoster: { ...roster, agents: [workerWithModel, pin] } }],
    items: [{ projectKey: "device/project", entity_id: "owner", agents: [pin] }] };
  expect(agentModelsFromFeed(feed, "device/project")[worker.id]).toEqual(workerWithModel);
});


it("rechecks dispatch capabilities after the cache fence is captured", async () => {
  let reads = 0;
  const call = vi.fn(async () => roster);
  await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities: () => ++reads === 1 ? capabilities : {}, call });
  expect(call).not.toHaveBeenCalled();
});
