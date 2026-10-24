import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import * as cache from "../src/core/localCache.js";
import { notePush } from "../src/core/pushFence.js";
import { projectAgentsAddress, syncProjectAgents } from "../src/core/projectAgentsCache.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const pin = { id: "pin" };
const worker = { id: "worker", conversation_id: "chat", thread_id: "thread", thread_generation_revision: 1 };
const roster = { entity_id: "owner", pinned_agent_id: "pin", agents: [pin, worker] };
const address = projectAgentsAddress("device", "owner");
const threadAddress = { deviceId: "device", entityId: "owner", kind: "thread", sub: "chat" };
const options = { deviceId: "device", entityId: "owner", capabilities: { agents: { projectWorkers: true } } };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
beforeEach(async () => { await cache.wipeCache(); });

it("direct rail and push refreshes make progress before a coalesced trailing read completes", async () => {
  const first = deferred(), trailing = deferred();
  const call = vi.fn().mockReturnValueOnce(first.promise).mockReturnValue(trailing.promise);
  const initial = syncProjectAgents({ ...options, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const followers = Array.from({ length: 20 }, () => syncProjectAgents({ ...options, call }));
  first.resolve(roster);
  await initial;
  expect((await cache.readCached(address))?.value).toEqual(roster);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  trailing.resolve(roster);
  await Promise.all(followers);
  expect(call).toHaveBeenCalledTimes(2);
});

async function pauseGenerationRead(run) {
  const read = cache.readCachedMany;
  const captured = deferred(), release = deferred();
  const spy = vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const records = await read(addresses);
    if (addresses.some((held) => held.kind === "thread")) { captured.resolve(); await release.promise; }
    return records;
  });
  try {
    const pending = syncProjectAgents({ ...options, call: async () => roster });
    await captured.promise;
    await run();
    release.resolve();
    await pending;
  } finally { release.resolve(); spy.mockRestore(); }
}

it("admits a roster while ordinary thread appends change the cached write stamp", async () => {
  await cache.writeCached(threadAddress, { thread_id: "thread", thread_generation_revision: 1, items: [] });
  await pauseGenerationRead(async () => {
    await cache.writeCached(threadAddress, { thread_id: "thread", thread_generation_revision: 1, items: [{ sequence: 1 }] });
    notePush(threadAddress);
  });
  expect((await cache.readCached(address))?.value).toEqual(roster);
});

it.each([
  { thread_id: "reset", thread_generation_revision: 2, retired_thread_ids: ["thread"] },
  { thread_id: "thread", thread_generation_revision: 2 },
  { thread_id: "thread", thread_generation_revision: 1, retired_thread_ids: ["thread"] },
  null,
])("blocks changed or removed generation evidence: %j", async (replacement) => {
  await cache.writeCached(threadAddress, { thread_id: "thread", thread_generation_revision: 1, items: [] });
  await pauseGenerationRead(() => replacement ? cache.writeCached(threadAddress, replacement) : cache.deleteCached([threadAddress]));
  expect((await cache.readCached(address))?.value).toBeFalsy();
});

it("keeps removal and epoch fences even when another reader is queued", async () => {
  let active = true;
  const answer = deferred();
  const call = vi.fn(() => answer.promise);
  const pending = syncProjectAgents({ ...options, call, active: () => active });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  active = false;
  answer.resolve(roster);
  await pending;
  expect((await cache.readCached(address))?.value).toBeFalsy();
  await syncProjectAgents({ ...options, call: async () => { notePush(address, { removed: true }); return roster; } });
  expect((await cache.readCached(address))?.value).toBeFalsy();
});
