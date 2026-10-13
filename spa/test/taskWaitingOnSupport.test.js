import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let support, cache;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  support = await import("../src/core/taskWaitingOnSupport.js");
  cache = await import("../src/core/localCache.js");
});

it("sets the highest-seen flag on a capable greeting, scoped per device", async () => {
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(false);
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  expect(await support.readTaskWaitingOnSupport("d")).toBe(true);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  expect(await support.readTaskWaitingOnEverSupported("other")).toBe(false);
});

it("keeps the highest-seen flag when a later greeting lacks support", async () => {
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  await support.rememberTaskWaitingOnSupport("d", { tasks: {} });
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  expect((await cache.readCached(support.taskWaitingOnSupportAddress("d"))).value)
    .toEqual({ waitingOn: false, everWaitingOn: true });
});

it("does not mark a never-upgraded device capable after an incapable greeting", async () => {
  await support.rememberTaskWaitingOnSupport("d", {});
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(false);
});

it("retains the flag across module reload and a subsequent incapable greeting", async () => {
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  vi.resetModules();
  support = await import("../src/core/taskWaitingOnSupport.js");
  await support.rememberTaskWaitingOnSupport("d", {});
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
});

it("carries an existing capable cache record into highest-seen history", async () => {
  await cache.writeCached(support.taskWaitingOnSupportAddress("d"), { waitingOn: true });
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  await support.rememberTaskWaitingOnSupport("d", {});
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
});

it("does not let a superseded greeting overwrite current support", async () => {
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  await support.rememberTaskWaitingOnSupport("d", {}, () => false);
  expect(await support.readTaskWaitingOnSupport("d")).toBe(true);
});

it("remembers an admitted capable greeting even if it is superseded before storage commits", async () => {
  let current = true;
  const first = support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } }, () => current);
  current = false;
  const second = support.rememberTaskWaitingOnSupport("d", {});
  await Promise.all([first, second]);
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
});

it("records historical evidence from a superseded capable greeting without rendering it", async () => {
  await support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } }, () => false);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  expect(await support.readTaskWaitingOnSupport("d")).toBe(false);
});

it("answers unknown when history storage is unavailable", async () => {
  globalThis.indexedDB = undefined;
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(null);
});

it("answers unknown for a malformed history record", async () => {
  await cache.writeCached(support.taskWaitingOnSupportAddress("d"), {});
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(null);
});

it("keeps failed-write history unknown until a committed retry, including no-op commits", async () => {
  const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
    throw new DOMException("Refused history", "QuotaExceededError");
  });
  try {
    expect(await support.rememberTaskWaitingOnSupport("d", {})).toBe(false);
    expect(await support.readTaskWaitingOnEverSupported("d")).toBe(null);
  } finally { put.mockRestore(); }
  expect(await support.rememberTaskWaitingOnSupport("d", {})).toBe(true);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(false);
  expect(await support.rememberTaskWaitingOnSupport("d", {})).toBe(true);
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(false);
});

it("updates capable evidence synchronously and retires pending history on account reset", async () => {
  const pending = support.rememberTaskWaitingOnSupport("d", { tasks: { waitingOn: true } });
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(true);
  support.resetTaskWaitingOnHistory();
  await pending;
  expect(await support.readTaskWaitingOnEverSupported("d")).toBe(false);
});
