import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let choices, cache, session, support, active;
const scope = { deviceId: "choices-891", projectId: "proj-1" };
const page = (values, next_cursor, dimension = "label") => ({ project_id: scope.projectId, dimension,
  choices: values, ...(next_cursor ? { next_cursor } : {}) });
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const reader = (rpc, dimension = "label", selectedScope = scope) => choices.createTaskFilterChoicesReader({
  scope: selectedScope, dimension, callRpc: rpc, capabilities: () => support,
  sessionOf: () => session, active: () => active,
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  session = {}; support = { filterChoices: true }; active = true;
  cache = await import("../src/core/localCache.js");
  choices = await import("../src/core/taskFilterChoices.js");
});

it("reads only the first page, permits empty continuation, and reads more only on demand", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["ui"], "a")).mockResolvedValueOnce(page([], "b"))
    .mockResolvedValueOnce(page(["api", "ui"]));
  const view = reader(rpc);
  expect(await view.read()).toBeNull();
  expect(await view.refresh()).toBe(true);
  expect(rpc.mock.calls).toEqual([["tasks.filterChoices", { project_id: scope.projectId, dimension: "label", limit: 100 }]]);
  expect((await view.read()).choices).toEqual(["ui"]);
  await view.more();
  expect((await view.read()).next_cursor).toBe("b");
  expect(rpc).toHaveBeenCalledTimes(2);
  await view.more();
  expect((await view.read()).choices).toEqual(["ui", "api"]);
  expect((await view.read()).complete).toBe(true);
  expect(await view.more()).toBe(false);
  expect(rpc.mock.calls[2][1].cursor).toBe("b");
  expect(rpc).toHaveBeenCalledTimes(3);
});

it("requires filterChoices independently of summary or legacy paging support", async () => {
  const rpc = vi.fn(); support = { listSummaries: true, listPaged: true };
  expect(await reader(rpc).refresh()).toBe(false);
  expect(rpc).not.toHaveBeenCalled();
  support = { filterChoices: true }; rpc.mockResolvedValue(page([]));
  expect(await reader(rpc).refresh()).toBe(true);
});

it("shares canonical membership across readers and keeps dimensions and projects independent", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["api"]))
    .mockResolvedValueOnce(page(["ready"], undefined, "status"));
  await reader(rpc).refresh(); await reader(rpc, "status").refresh();
  expect((await reader(vi.fn()).read()).choices).toEqual(["api"]);
  expect((await reader(vi.fn(), "status").read()).choices).toEqual(["ready"]);
  expect(await reader(vi.fn(), "label", { ...scope, projectId: "proj-2" }).read()).toBeNull();
  expect(await reader(vi.fn(), "label", { ...scope, deviceId: "device-2" }).read()).toBeNull();
  expect(choices.filterChoicesAddress(scope, "label")).toEqual({ deviceId: scope.deviceId,
    entityId: scope.projectId, kind: "tracker-filter-choices", sub: "label" });
});

it("deduplicates equivalent actor objects while preserving opaque ID case and server order", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["none", { kind: "agent", agent_id: "AgentA" }], "a", "assignee"))
    .mockResolvedValueOnce(page([{ agent_id: "AgentA", kind: "agent" }, { kind: "agent", agent_id: "agenta" },
      { kind: "user" }, "none"], undefined, "assignee"));
  const view = reader(rpc, "assignee"); await view.refresh(); await view.more();
  expect((await view.read()).choices).toEqual(["none", { kind: "agent", agent_id: "AgentA" },
    { kind: "agent", agent_id: "agenta" }, { kind: "user" }]);
});

it("deduplicates labels with backend ASCII folding while retaining non-ASCII distinctions", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["API", "É"], "a"))
    .mockResolvedValueOnce(page(["api", "é", "UI"]));
  const view = reader(rpc); await view.refresh(); await view.more();
  expect((await view.read()).choices).toEqual(["API", "É", "é", "UI"]);
});

it("coalesces more presses and preserves the cached cursor after a terminal RPC error", async () => {
  const pending = deferred();
  const rpc = vi.fn().mockResolvedValueOnce(page(["api"], "a")).mockImplementationOnce(() => pending.promise)
    .mockRejectedValueOnce(Object.assign(new Error("oversize"), { retryable: false }));
  const view = reader(rpc); await view.refresh();
  const one = view.more(), two = view.more();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2)); pending.resolve(page([], "b"));
  expect(await one).toBe(true); expect(await two).toBe(true);
  const held = await view.read();
  await expect(view.more()).rejects.toMatchObject({ retryable: false });
  expect(await view.read()).toEqual(held);
});

it("keeps prior data and usable continuation after a failed refresh", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["api"], "a")).mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValueOnce(page(["ui"]));
  const view = reader(rpc); await view.refresh();
  await expect(view.refresh()).rejects.toThrow("offline");
  expect(await view.read()).toMatchObject({ choices: ["api"], next_cursor: "a", refreshing: false });
  expect(await view.more()).toBe(true);
  expect((await view.read()).choices).toEqual(["api", "ui"]);
});

it("fences older refreshes across reader instances and replaces membership only on successful first page", async () => {
  const pending = deferred();
  const oldRpc = vi.fn(() => pending.promise), newer = reader(vi.fn().mockResolvedValue(page(["new"])));
  const older = reader(oldRpc), first = older.refresh();
  await vi.waitFor(() => expect(oldRpc).toHaveBeenCalledTimes(1));
  await newer.refresh(); pending.resolve(page(["old"], "old-cursor"));
  expect(await first).toBe(false);
  expect((await older.read()).choices).toEqual(["new"]);
  expect(await older.more()).toBe(false);
});

it.each(["session", "account", "capability", "dispose"])("fences a pending page after %s retirement", async (kind) => {
  const pending = deferred(), rpc = vi.fn(() => pending.promise), view = reader(rpc);
  const refresh = view.refresh(); await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  if (kind === "session") session = {};
  if (kind === "account") active = false;
  if (kind === "capability") support = {};
  if (kind === "dispose") view.dispose();
  pending.resolve(page(["retired"])); expect(await refresh).toBe(false);
  expect((await view.read()).choices).toEqual([]);
  expect(await view.more()).toBe(false);
});

it("checks acceptance inside the IndexedDB transaction for reservation and page publication", async () => {
  const accepted = vi.fn(() => false);
  expect(await choices.beginFilterChoices(scope, "label", 1, { accept: accepted })).toBeNull();
  expect(accepted).toHaveBeenCalled(); expect(await choices.readFilterChoices(scope, "label")).toBeNull();
  const ticket = await choices.beginFilterChoices(scope, "label", 2);
  expect(await choices.landFilterChoices(ticket, page(["rejected"]), { accept: accepted })).toBe(false);
  expect((await choices.readFilterChoices(scope, "label")).choices).toEqual([]);
});

it("rechecks a captured account guard after the page transaction has begun", async () => {
  const ticket = await choices.beginFilterChoices(scope, "label", 1);
  let accepts = true;
  const original = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    accepts = false;
    return original.call(this, key);
  });
  expect(await choices.landFilterChoices(ticket, page(["retired"]), { accept: () => accepts })).toBe(false);
  spy.mockRestore();
  expect((await choices.readFilterChoices(scope, "label")).choices).toEqual([]);
});

it("leaves membership and cursor atomic when IndexedDB aborts a page write", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(page(["api"], "a")).mockResolvedValueOnce(page(["ui"]));
  const view = reader(rpc); await view.refresh(); const before = await view.read();
  const original = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (value, key) {
    if (value?.value?.choices?.includes("ui")) this.transaction.abort();
    return original.call(this, value, key);
  });
  expect(await view.more()).toBe(false); spy.mockRestore();
  expect(await view.read()).toEqual(before);
});

it("rejects invalid dimensions, mismatched pages and repeated cursors without consuming cache", async () => {
  expect(() => reader(vi.fn(), "workspace")).toThrow();
  const rpc = vi.fn().mockResolvedValueOnce(page(["api"], "a"))
    .mockResolvedValueOnce(page(["ui"], "a")).mockResolvedValueOnce(page(["ready"], undefined, "status"))
    .mockResolvedValueOnce({ ...page(["other"]), project_id: "proj-2" });
  const view = reader(rpc); await view.refresh(); const before = await view.read();
  await expect(view.more()).rejects.toThrow(); await expect(view.more()).rejects.toThrow();
  await expect(view.more()).rejects.toThrow(); expect(await view.read()).toEqual(before);
  expect((await cache.readCached(choices.filterChoicesAddress(scope, "label"))).value.choices).toEqual(["api"]);
});
