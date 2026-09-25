import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

const timing = { reopenDelaysMs: [0, 1, 2, 3], restMs: 20, openTimeoutMs: 200, giveUpAfterMs: 60000 };
const count = { deviceId: "", entityId: "", kind: "review-count" };
const list = { deviceId: "d", entityId: "p", kind: "tracker-issues" };
const ledger = { deviceId: "d", entityId: "p", kind: "tracker-issue-reads" };
const lost = () => new DOMException("Connection to Indexed Database server lost", "UnknownError");
let cache;

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  cache.setCacheRecoveryTiming(timing);
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => vi.restoreAllMocks());

it("keeps count and joint merge pending across a temporary open outage", async () => {
  await cache.writeCached(list, ["old"]);
  await cache.writeCached(ledger, ["old"]);
  const transaction = IDBDatabase.prototype.transaction;
  let first = true;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (first) {
      first = false;
      throw lost();
    }
    return transaction.apply(this, args);
  });

  const open = indexedDB.open.bind(indexedDB);
  let failing = true;
  vi.spyOn(indexedDB, "open").mockImplementation((...args) => {
    if (!failing) return open(...args);
    const request = { error: lost() };
    setTimeout(() => request.onerror?.({ preventDefault() {} }));
    return request;
  });

  const answers = [];
  const taken = cache.takeCachedCount(count).then((value) => {
    answers.push(value);
    return value;
  });
  await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
  const merged = cache.mergeCachedTogether([list, ledger], () => [["new"], ["new"]]).then((value) => {
    answers.push(value);
    return value;
  });
  await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
  expect(answers).toEqual([]);
  failing = false;

  expect(await taken).toBe(1);
  expect(await merged).toBe(true);
  expect((await cache.readCachedMany([list, ledger])).map((record) => record.value))
    .toEqual([["new"], ["new"]]);
});

it("retries a joint merge atomically when a partial put loses connection", async () => {
  await cache.writeCached(list, ["old"]);
  await cache.writeCached(ledger, ["old"]);
  const put = IDBObjectStore.prototype.put;
  let failures = 0;
  let callbacks = 0;
  vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (value, key) {
    if (String(key).includes("tracker-issue-reads") && failures < 3) {
      failures += 1;
      throw lost();
    }
    return put.call(this, value, key);
  });

  const heard = [];
  cache.subscribeCache({ deviceId: "d" }, () => heard.push(true));
  expect(await cache.mergeCachedTogether([list, ledger], (values) => {
    callbacks += 1;
    expect(values).toEqual([["old"], ["old"]]);
    return [["new"], ["new"]];
  })).toBe(true);
  expect(callbacks).toBe(4);
  expect(heard).toHaveLength(2);
  expect((await cache.readCachedMany([list, ledger])).map((record) => record.value))
    .toEqual([["new"], ["new"]]);
});

it("keeps later joint merges behind an earlier merge waiting for recovery", async () => {
  cache.setCacheRecoveryTiming({ ...timing, restMs: 200 });
  await cache.writeCached(list, ["old"]);
  await cache.writeCached(ledger, ["old"]);
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < timing.reopenDelaysMs.length) throw lost();
    return transaction.apply(this, args);
  });
  const first = cache.mergeCachedTogether([list, ledger], () => [["first"], ["first"]]);
  await vi.waitFor(() => expect(failures).toBe(timing.reopenDelaysMs.length), { interval: 1 });
  const second = cache.mergeCachedTogether([list, ledger], () => [["second"], ["second"]]);

  expect(await Promise.all([first, second])).toEqual([true, true]);
  expect((await cache.readCachedMany([list, ledger])).map((record) => record.value))
    .toEqual([["second"], ["second"]]);
});

it("keeps a newer tab's folded row when an older fold resumes after recovery", async () => {
  cache.setCacheRecoveryTiming({ ...timing, restMs: 200 });
  await cache.readCached(list);
  const oldPages = await import("../src/core/trackerPages.js");
  vi.resetModules();
  const otherPages = await import("../src/core/trackerPages.js");
  const oldRow = { id: "same", number: 2, title: "old", updated_at: "2026-01-01" };
  const newRow = { ...oldRow, title: "new" };
  const stretch = (row, read) => ({
    issues: [row], above: Infinity, through: -Infinity,
    read, readAt: Date.parse("2026-01-01"), pullRead: read,
  });
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < timing.reopenDelaysMs.length) throw lost();
    return transaction.apply(this, args);
  });

  const older = oldPages.foldIssuesPage(list, stretch(oldRow, 1), () => []);
  await vi.waitFor(() => expect(failures).toBe(timing.reopenDelaysMs.length), { interval: 1 });
  expect(await otherPages.foldIssuesPage(list, stretch(newRow, 2), () => [])).toBe(true);
  expect(await older).toBe(true);
  expect((await cache.readCached(list))?.value.issues[0].title).toBe("new");
});

it("does not skip a page whose canonical fold loses its connection through a retry round", async () => {
  const { foldIssuesPage, pullIssuePages } = await import("../src/core/trackerPages.js");
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  let folds = 0;
  const pages = [
    { issues: [{ id: "first", number: 2, title: "first", updated_at: "2026-01-01" }], next_cursor: "second-page" },
    { issues: [{ id: "second", number: 1, title: "second", updated_at: "2026-01-01" }] },
  ];
  const completed = await pullIssuePages({
    deviceId: "d", projectId: "p", params: {},
    ask: async () => pages.shift(),
    fold: async (stretch) => {
      folds += 1;
      if (folds === 1) vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
        if (failures++ < 4) throw lost();
        return transaction.apply(this, args);
      });
      return foldIssuesPage(list, stretch, () => []);
    },
  });
  expect(completed).toBe(true);
  expect((await cache.readCached(list))?.value.issues.map((row) => row.id)).toEqual(["first", "second"]);
});

it("stops at a page whose canonical fold was refused", async () => {
  const { pullIssuePages } = await import("../src/core/trackerPages.js");
  const ask = vi.fn(async () => ({
    issues: [{ id: "first", number: 2, title: "first", updated_at: "2026-01-01" }],
    next_cursor: "second-page",
  }));

  expect(await pullIssuePages({
    deviceId: "d", projectId: "p", params: {}, ask,
    fold: async () => false,
  })).toBe(false);
  expect(ask).toHaveBeenCalledTimes(1);
  expect(await cache.readCached(list)).toBeUndefined();
});
