// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { refreshReadState } from "../src/core/readStateRepair.js";
import { notePush, forgetPushes } from "../src/core/pushFence.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";

const address = (entityId, kind = "row") => ({ deviceId: "repair-device", entityId, kind, sub: "" });
const row = (cursor, extra = {}) => ({ run_id: "owner", unread_count: 12 - cursor, can_finish: true,
  agents: [{ id: "agent", conversation_id: "chat", thread_id: "generation", read_through_sequence: cursor,
    unread_count: 12 - cursor, surfaces: { goal: { text: "keep detail" } } }], ...extra });
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const context = (call, active = () => true) => ({ deviceId: "repair-device", session: {}, requestScope: {}, call, active });

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  forgetPushes();
  await wipeCache();
});
afterEach(async () => { await wipeCache(); });

async function seed() {
  await writeCached(address("", "feed"), { items: [row(10)], runs: [], pending: [{ id: "keep" }] });
  await writeCached(address("owner"), row(10));
  await writeCached(address("", "projects"), []);
}

it("repairs read fields without replacing ownership, row detail or observation times", async () => {
  await seed();
  const before = await readCached(address("", "feed"));
  const call = vi.fn(async () => ({ items: [row(12, { can_finish: false }), { run_id: "new-owner", agents: [] }] }));
  expect(await refreshReadState(context(call))).toBe(true);
  expect(call.mock.calls).toEqual([["board.list", {}]]);
  expect((await readCached(address("owner"))).value).toEqual(row(12));
  const feed = (await readCached(address("", "feed"))).value;
  expect(feed.items).toHaveLength(1);
  expect(feed.items[0].unread_count).toBe(0);
  expect(feed.items[0].__cacheObserved).toEqual(before.value.items[0].__cacheObserved || { at: before.at, order: before.order });
  expect(feed.pending).toEqual([{ id: "keep" }]);
});

it("keeps a pushed row and feed newer than its board reply", async () => {
  await seed();
  const held = deferred();
  const call = vi.fn(() => held.promise);
  const repair = refreshReadState(context(call));
  await vi.waitFor(() => expect(call).toHaveBeenCalled());
  const pushed = row(11, { needs_attention: true });
  await writeCached(address("owner"), pushed);
  notePush(address("owner"));
  await writeCached(address("", "feed"), { items: [pushed] });
  notePush(address("", "feed"));
  held.resolve({ items: [row(12)] });
  await repair;
  expect((await readCached(address("owner"))).value).toEqual(pushed);
  expect((await readCached(address("", "feed"))).value.items[0].unread_count).toBe(1);
});

it("coalesces a burst into one active and one trailing background repair", async () => {
  await seed();
  const held = deferred();
  const call = vi.fn().mockReturnValueOnce(held.promise).mockResolvedValue({ items: [row(12)] });
  const owner = context(call);
  const first = refreshReadState(owner);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  const rest = Array.from({ length: 8 }, () => refreshReadState(owner));
  held.resolve({ items: [row(11)] });
  expect(await Promise.all([first, ...rest])).toEqual(Array(9).fill(true));
  expect(call).toHaveBeenCalledTimes(2);
  expect((await readCached(address("owner"))).value.unread_count).toBe(0);
});

it("lets a replacement session bypass an old stalled repair and rejects its stale reply", async () => {
  await seed();
  let current = true;
  const held = deferred();
  const oldCall = vi.fn(() => held.promise);
  const old = refreshReadState(context(oldCall, () => current));
  await vi.waitFor(() => expect(oldCall).toHaveBeenCalled());
  current = false;
  const call = vi.fn(async () => ({ items: [row(12)] }));
  expect(await refreshReadState(context(call))).toBe(true);
  held.resolve({ items: [row(11)] });
  expect(await old).toBe(false);
  expect((await readCached(address("owner"))).value.unread_count).toBe(0);
});

it("refreshes modern project-owner badges through the complete project roster", async () => {
  await seed();
  await writeCached(address("", "projects"), [{ project_id: "project", entity_id: "owner" }]);
  const roster = { entity_id: "owner", pinned_agent_id: "agent", agents: row(12).agents };
  const call = vi.fn(async (method) => method === "agent.list" ? roster : { items: [row(12)] });
  const owner = { ...context(call), capabilities: { agents: { projectWorkers: true } } };
  expect(await refreshReadState(owner)).toBe(true);
  expect(call.mock.calls).toContainEqual(["agent.list", { entity_id: "owner", include_project_workers: true }]);
  expect((await readCached(address("owner", "project-agents"))).value).toEqual(roster);
});

it("does not merge read cursors across a thread generation change", async () => {
  await seed();
  const incoming = row(12);
  incoming.agents[0].thread_generation_revision = 2;
  await refreshReadState(context(async () => ({ items: [incoming] })));
  expect((await readCached(address("owner"))).value.unread_count).toBe(2);
});

it("does not request project-worker rosters for ordinary workspace read marks", async () => {
  await seed();
  const { repairReadOwner } = await import("../src/core/readStateRepair.js");
  const call = vi.fn();
  await repairReadOwner({ ...context(call), capabilities: { agents: { projectWorkers: true } } }, "owner");
  expect(call).not.toHaveBeenCalled();
});
