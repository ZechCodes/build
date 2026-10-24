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
const options = { background: true, deviceId: "device", entityId: "owner", capabilities: { agents: { projectWorkers: true } } };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
beforeEach(async () => { await cache.wipeCache(); });

it("background refreshes make progress before a coalesced trailing read completes", async () => {
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

it("keeps removal and epoch fences", async () => {
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

it("a navigation-cancelled trailing rail read does not discard active push discovery", async () => {
  let standing = true;
  const first = deferred();
  const call = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(roster);
  const initial = syncProjectAgents({ ...options, call, active: () => standing });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const afterWrite = vi.fn();
  const push = syncProjectAgents({ ...options, call, afterWrite });
  const rail = syncProjectAgents({ ...options, call, active: () => standing });
  standing = false;
  first.resolve(roster);
  await Promise.all([initial, push, rail]);
  expect((await cache.readCached(address))?.value).toEqual(roster);
  expect(afterWrite).toHaveBeenCalledExactlyOnceWith(roster);
  expect(call).toHaveBeenCalledTimes(2);
});

it("a queued read retains the entity removal fence from its invocation", async () => {
  const first = deferred();
  const call = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(roster);
  const initial = syncProjectAgents({ ...options, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const queued = syncProjectAgents({ ...options, call });
  notePush(address, { removed: true });
  await cache.deleteCached([address]);
  first.resolve(roster);
  await Promise.all([initial, queued]);
  expect((await cache.readCached(address))?.value).toBeFalsy();
});

it("releases the lane after an RPC failure and keeps its trailing refresh", async () => {
  const first = deferred();
  const call = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(roster);
  const initial = syncProjectAgents({ ...options, call });
  const rejected = expect(initial).rejects.toThrow("offline");
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const queued = syncProjectAgents({ ...options, call });
  first.reject(new Error("offline"));
  await rejected;
  await queued;
  expect((await cache.readCached(address))?.value).toEqual(roster);
});


it("an immediate user refresh admits its roster while background RPCs remain held", async () => {
  const background = deferred();
  const call = vi.fn(() => background.promise);
  const initial = syncProjectAgents({ ...options, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const queued = syncProjectAgents({ ...options, call });
  const foregroundCall = vi.fn(async () => roster);
  const immediate = syncProjectAgents({ ...options, background: false, call: foregroundCall });
  try {
    await vi.waitFor(() => expect(foregroundCall).toHaveBeenCalledTimes(1));
    await immediate;
    expect((await cache.readCached(address))?.value).toEqual(roster);
  } finally {
    background.resolve({ ...roster, agents: [pin] });
    await Promise.all([initial, queued, immediate]);
  }
  expect((await cache.readCached(address))?.value).toEqual(roster);
});

it("retries discovery with a live push subscriber when the selected rail unmounts after dispatch", async () => {
  const first = deferred(), railReply = deferred();
  let standing = true;
  const call = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(roster);
  const initial = syncProjectAgents({ ...options, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const push = syncProjectAgents({ ...options, call });
  const railCall = vi.fn(() => railReply.promise);
  const rail = syncProjectAgents({ ...options, call: railCall, active: () => standing });
  first.resolve({ ...roster, agents: [pin] });
  await initial;
  await vi.waitFor(() => expect(railCall).toHaveBeenCalledTimes(1));
  standing = false;
  railReply.resolve(roster);
  await Promise.all([push, rail]);
  expect((await cache.readCached(address))?.value).toEqual(roster);
  expect(call).toHaveBeenCalledTimes(2);
});


it("background calls starting after an immediate read cannot supersede its admission", async () => {
  const reply = deferred(), backgroundReply = deferred();
  const foregroundCall = vi.fn(() => reply.promise);
  const immediate = syncProjectAgents({ ...options, background: false, call: foregroundCall });
  await vi.waitFor(() => expect(foregroundCall).toHaveBeenCalledTimes(1));
  const call = vi.fn(() => backgroundReply.promise);
  const background = Array.from({ length: 20 }, () => syncProjectAgents({ ...options, call }));
  reply.resolve(roster);
  await immediate;
  expect((await cache.readCached(address))?.value).toEqual(roster);
  backgroundReply.resolve(roster);
  await Promise.all(background);
  expect(call).toHaveBeenCalledTimes(1);
});

it("keeps an invalidation arriving during covered-cache reading as a trailing obligation", async () => {
  const read = cache.readCached;
  const captured = deferred(), release = deferred();
  const spy = vi.spyOn(cache, "readCached").mockImplementation(async (address) => {
    const record = await read(address);
    captured.resolve();
    await release.promise;
    return record;
  });
  const call = vi.fn(async () => roster);
  try {
    const early = syncProjectAgents({ ...options, call });
    const immediate = syncProjectAgents({ ...options, background: false, call: async () => roster });
    await captured.promise;
    const late = syncProjectAgents({ ...options, call });
    release.resolve();
    await Promise.all([early, immediate, late]);
    expect(call).toHaveBeenCalledTimes(1);
  } finally { release.resolve(); spy.mockRestore(); }
});


it("unmarked calls stay immediate and keep their existing latest-reader fence", async () => {
  const first = deferred(), second = deferred();
  const call = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  const initial = syncProjectAgents({ ...options, background: false, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const latest = syncProjectAgents({ ...options, background: false, call });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  first.resolve(roster);
  await initial;
  expect((await cache.readCached(address))?.value).toBeFalsy();
  second.resolve(roster);
  await latest;
  expect((await cache.readCached(address))?.value).toEqual(roster);
});
