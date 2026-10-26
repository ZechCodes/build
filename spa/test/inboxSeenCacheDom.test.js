/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const call = vi.fn();
let activeContext = null;
vi.mock("../src/core/inboxDevices.js", () => ({ verbCall: () => call }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => activeContext, homeContext: () => ({ deviceId: "dev-1" }) }));
const { markSeen } = await import("../src/core/inboxSeen.js");
const { cachedWriteOf, readCached, subscribeCache, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { stampRow } = await import("../src/core/feedMerge.js");
const { branchCloseout } = await import("../src/core/branchFinish.js");
const address = { deviceId: "dev-1", entityId: "run-1", kind: "row", sub: "" };
const roster = (cursor) => ({
  kind: "project", run_id: "run-1",
  agents: [{ id: "agent-1", thread_id: "thread-1", read_through_sequence: cursor, unread_count: 12 - cursor }],
});
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
beforeEach(async () => {
  call.mockReset();
  activeContext = null;
  await wipeCache();
});
afterEach(async () => { await wipeCache(); });

it("keeps the latest observed read cursor when overlapping older-bridge roster refreshes finish", async () => {
  await writeCached(address, roster(10));
  const older = deferred();
  const newer = deferred();
  let pulls = 0;
  call.mockImplementation(async (method) => {
    if (method === "entity.seen") return { ok: true };
    pulls += 1;
    return pulls === 1 ? older.promise : newer.promise;
  });
  const first = markSeen("run-1", "agent-1", 1, 11, "thread-1", "dev-1");
  await vi.waitFor(() => expect(pulls).toBe(1));
  const second = markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1");
  // Let the second RPC and its pre-read capture happen before the older reply.
  await new Promise((done) => setTimeout(done, 20));
  older.resolve({ items: [], runs: [roster(11)] });
  await first;
  await vi.waitFor(() => expect(pulls).toBe(2));
  newer.resolve({ items: [], runs: [roster(12)] });
  await second;
  expect((await readCached(address)).value.agents[0].unread_count).toBe(0);
});

it("preserves unwatched owner detail and closeout when only a List digest names its read cursor", async () => {
  const detail = stampRow({
    ...roster(11), kind: "branch", project_id: "p1", branch: "build/test", state: "building", can_finish: true,
    finish: { warnings: ["Unpublished commits"] },
  }, "dev-1");
  Object.assign(detail.agents[0], {
    watched: false, conversation_id: "conversation-1", surface_session_generation: "session-4",
    surfaces: { goal: { text: "review task" } },
  });
  expect(branchCloseout(detail).shown).toBe(true);
  await writeCached(address, detail);
  const listed = {
    ...roster(12), project_id: "p1", branch: "build/test", state: "building", can_finish: false,
  };
  delete listed.kind;
  Object.assign(listed.agents[0], {
    watched: false, conversation_id: "conversation-1", surface_session_generation: "session-4",
  });
  call.mockImplementation(async (method) => method === "entity.seen" ? { ok: true } : { items: [], runs: [listed] });
  await expect(markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1")).resolves.toBe(true);
  const after = (await readCached(address)).value;
  expect.soft(after.agents[0].unread_count).toBe(0);
  expect.soft(after.agents[0].read_through_sequence).toBe(12);
  expect.soft(after.agents[0].surfaces).toEqual(detail.agents[0].surfaces);
  expect.soft(after.can_finish).toBe(true);
  expect.soft(after.finish).toEqual(detail.finish);
  expect.soft(branchCloseout(after).shown).toBe(true);
});

it("keeps the write stamp and sends no announcement when the pushed read roster is already current", async () => {
  const pushed = stampRow(roster(12), "dev-1");
  await writeCached(address, pushed);
  const before = await readCached(address);
  const announce = vi.fn();
  const unsubscribe = subscribeCache(address, announce);
  call.mockImplementation(async (method) => method === "entity.seen" ? { ok: true } : { items: [], runs: [roster(12)] });
  try {
    await expect(markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1")).resolves.toBe(true);
    const after = await readCached(address);
    expect.soft(after.value).toEqual(before.value);
    expect.soft(cachedWriteOf(after)).toBe(cachedWriteOf(before));
    expect.soft(announce).not.toHaveBeenCalled();
  } finally {
    unsubscribe();
  }
});

it("announces each markSeen row write immediately while project-owner repair is held", async () => {
  activeContext = { session: {}, cacheScope: {}, active: () => true,
    adapter: { capabilities: { agents: { projectWorkers: true } } } };
  await writeCached(address, roster(10));
  await writeCached({ ...address, kind: "project-agents" }, {
    entity_id: "run-1", pinned_agent_id: "agent-1", agents: roster(10).agents,
  });
  const repair = deferred();
  let cursor = 11;
  call.mockImplementation(async (method) => {
    if (method === "entity.seen") return { ok: true };
    if (method === "board.list") return { runs: [roster(cursor)] };
    return repair.promise;
  });
  const announce = vi.fn();
  const unsubscribe = subscribeCache(address, announce);
  try {
    await expect(markSeen("run-1", "agent-1", 1, 11, "thread-1", "dev-1")).resolves.toBe(true);
    await vi.waitFor(() => expect(call.mock.calls.filter(([method]) => method === "agent.list")).toHaveLength(1));
    expect((await readCached(address)).value.agents[0].read_through_sequence).toBe(11);
    expect(announce).toHaveBeenCalledTimes(1);
    cursor = 12;
    await expect(markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1")).resolves.toBe(true);
    expect((await readCached(address)).value.agents[0].read_through_sequence).toBe(12);
    expect(announce).toHaveBeenCalledTimes(2);
    expect(call.mock.calls.filter(([method]) => method === "entity.seen")).toHaveLength(2);
    expect(call.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(2);
  } finally {
    repair.resolve({ entity_id: "run-1", pinned_agent_id: "agent-1", agents: roster(12).agents });
    await vi.waitFor(async () => expect((await readCached({ ...address, kind: "project-agents" }))?.value?.agents[0].read_through_sequence).toBe(12));
    unsubscribe();
  }
});

it.each(["older-first", "newer-first"])("keeps a newer markSeen row and feed cursor across a competing read repair (%s)", async (order) => {
  const { refreshReadState } = await import("../src/core/readStateRepair.js");
  const feedAddress = { ...address, entityId: "", kind: "feed" };
  await writeCached(address, roster(10));
  await writeCached(feedAddress, { runs: [roster(10)] });
  const older = deferred();
  const newer = deferred();
  const oldCall = vi.fn(() => older.promise);
  const first = refreshReadState({ deviceId: "dev-1", call: oldCall, active: () => true, session: {}, requestScope: {} });
  await vi.waitFor(() => expect(oldCall).toHaveBeenCalled());
  call.mockImplementation(async (method) => method === "entity.seen" ? { ok: true } : newer.promise);
  const second = markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1");
  await vi.waitFor(() => expect(call.mock.calls.filter(([method]) => method === "board.list")).toHaveLength(1));
  if (order === "older-first") {
    older.resolve({ runs: [roster(11)] });
    await first;
    newer.resolve({ runs: [roster(12)] });
  } else {
    newer.resolve({ runs: [roster(12)] });
    await second;
    await vi.waitFor(async () => expect((await readCached(feedAddress)).value.runs[0].agents[0].read_through_sequence).toBe(12));
    older.resolve({ runs: [roster(11)] });
  }
  await Promise.all([first, second]);
  expect((await readCached(address)).value.agents[0].read_through_sequence).toBe(12);
  await vi.waitFor(async () => expect((await readCached(feedAddress)).value.runs[0].agents[0].read_through_sequence).toBe(12));
});

it("repairs a legacy feed-only owner after markSeen without creating a standalone row", async () => {
  const feedAddress = { ...address, entityId: "", kind: "feed" };
  await writeCached(feedAddress, { runs: [roster(10)] });
  call.mockImplementation(async (method) => method === "entity.seen" ? { ok: true } : { runs: [roster(12)] });
  expect(await markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1")).toBe(true);
  expect(call.mock.calls.map(([method]) => method)).toEqual(["entity.seen", "board.list"]);
  await vi.waitFor(async () => expect((await readCached(feedAddress)).value.runs[0].agents[0].unread_count).toBe(0));
  expect((await readCached(address))?.value).toBeFalsy();
});
