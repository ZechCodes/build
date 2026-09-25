// #169: on iOS every surface went blank after the app resumed. WebKit drops a
// suspended page's IndexedDB connection, the first opens and transactions
// after the resume fail with "Connection to Indexed Database server lost", and
// the cache — which retried once — stood down for the session. These drive
// that failure through fake-indexeddb: the cache rides it out, stands down
// only for what no reopen can fix, and records each step where a phone can
// show it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore, forceCloseDatabase } from "fake-indexeddb";

const FAST_RECOVERY = { reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40, blockedTimeoutMs: 1000 };
const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };

let cache;
let diagnostics;

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  diagnostics = await import("../src/core/connectionDiagnostics.js");
  cache.setCacheRecoveryTiming(FAST_RECOVERY);
  vi.spyOn(console, "info").mockImplementation(() => {});
});

const cacheEvents = () =>
  diagnostics.connectionDiagnosticHistory().filter((entry) => entry.connection === cache.CACHE_DIAGNOSTIC);
const eventNames = () => cacheEvents().map((entry) => entry.event);

const connectionLost = () => new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError");

/** An open request that fails the way WebKit's does: asynchronously, through
 *  `onerror`, with the error on the request. */
function failingOpenRequest(error) {
  const request = { error, result: undefined };
  setTimeout(() => request.onerror?.({ preventDefault() {} }));
  return request;
}

/** `indexedDB.open` failing its first `times` calls with `error`. */
function failOpens(times, error = connectionLost()) {
  const originalOpen = indexedDB.open.bind(indexedDB);
  let failures = 0;
  return vi.spyOn(indexedDB, "open").mockImplementation((...args) => {
    if (failures < times) {
      failures += 1;
      return failingOpenRequest(error);
    }
    return originalOpen(...args);
  });
}

describe("a connection lost across a resume", () => {
  it("reopens through several failed opens without standing down", async () => {
    const handles = [];
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      return originalTransaction.apply(this, args);
    });
    await cache.writeCached(address, { head: "before-sleep" });
    transaction.mockRestore();
    // The page is suspended: WebKit closes the connection, and the next few
    // opens after the resume find its storage server gone.
    forceCloseDatabase(handles[0]);
    const open = failOpens(3);
    const warn = vi.spyOn(console, "warn");

    expect((await cache.readCached(address))?.value).toEqual({ head: "before-sleep" });
    expect(open).toHaveBeenCalledTimes(4);
    expect(cache.cacheHealth().state).toBe("ready");
    expect(warn).not.toHaveBeenCalled();
    expect(eventNames()).toEqual(["cache-connection-closed", "cache-connection-lost", "cache-recovered"]);
    expect(cacheEvents()[1]).toMatchObject({
      reason: "open-failed",
      error: "UnknownError",
      message: expect.stringContaining("Connection to Indexed Database server lost"),
      visibility: "unknown",
      sinceShownMs: null,
    });
    expect(cacheEvents()[2]).toMatchObject({ attempts: 3, afterMs: expect.any(Number) });
  });

  it("retries a transaction the lost connection keeps failing, and announces only the commit", async () => {
    await cache.writeCached(address, { head: "before" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failures = 0;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (failures < 3) {
        failures += 1;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });

    await cache.writeCached(address, { head: "after" });

    expect(failures).toBe(3);
    expect(heard).toHaveLength(1);
    expect((await cache.readCached(address))?.value).toEqual({ head: "after" });
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-recovered"]);
    expect(cacheEvents()[0]).toMatchObject({ reason: "transaction-failed", error: "UnknownError" });
  });

  it("rides out the error real WebKit raised when its storage process died under a transaction", async () => {
    // Playwright WebKit, the network process killed with reads and writes in
    // flight: the old cache matched only "connection … lost" and stood down on
    // this at once, answering nothing to every later read.
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("An internal error was encountered in the Indexed Database server", "UnknownError");
      }
      return originalTransaction.apply(this, args);
    });
    expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
    expect(cache.cacheHealth().state).toBe("ready");
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-recovered"]);
  });

  it("rests when every open fails, keeps readers waiting, and answers them when the rest is over", async () => {
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });
    const open = failOpens(FAST_RECOVERY.reopenDelaysMs.length + 2);
    let answered = false;
    const read = cache.readCached(address).then((record) => {
      answered = true;
      return record;
    });

    await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
    expect(answered).toBe(false);
    // Another reader asking during the rest waits with the first.
    const second = cache.readCached(address);

    expect((await read)?.value).toEqual({ head: "stored" });
    expect((await second)?.value).toEqual({ head: "stored" });
    expect(open.mock.calls.length).toBe(FAST_RECOVERY.reopenDelaysMs.length + 3);
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-resting", "cache-recovered"]);
    expect(cacheEvents()[1]).toMatchObject({ forMs: FAST_RECOVERY.restMs, error: "UnknownError" });
  });

  it("keeps two writes to one record in the order they were asked when the first is retried", async () => {
    // A push writes "running", and its transaction dies with the connection;
    // the next push writes "done" while the first waits to retry. The retry
    // must not land on top of the newer write.
    cache.setCacheRecoveryTiming({ ...FAST_RECOVERY, reopenDelaysMs: [0, 50, 100, 150] });
    await cache.writeCached(address, { status: "queued" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });

    const running = cache.writeCached(address, { status: "running" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    const done = cache.writeCached(address, { status: "done" });
    await Promise.all([running, done]);

    expect((await cache.readCached(address))?.value).toEqual({ status: "done" });
  });

  it("makes a write asked for while the cache rests once the database is back", async () => {
    await cache.writeCached(address, { head: "before" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });
    failOpens(FAST_RECOVERY.reopenDelaysMs.length);
    const write = cache.writeCached(address, { head: "during" });
    await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
    expect(heard).toEqual([]);

    await write;

    expect(heard).toHaveLength(1);
    expect((await cache.readCached(address))?.value).toEqual({ head: "during" });
  });
});

describe("a database that never comes back", () => {
  // Safari has UnknownErrors that are not weather: a store it cannot open or
  // migrate fails every open, the same way, forever. Waiting on it held every
  // read — and the boot paint behind them — until the site's data was cleared.

  it("stands down after failing in plain sight past the ceiling, and answers the readers", async () => {
    cache.setCacheRecoveryTiming({ ...FAST_RECOVERY, giveUpAfterMs: 100 });
    const handles = [];
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      return originalTransaction.apply(this, args);
    });
    await cache.writeCached(address, { head: "stored" });
    transaction.mockRestore();
    forceCloseDatabase(handles[0]);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    failOpens(Infinity, new DOMException("Error creating or migrating Records table in database", "UnknownError"));

    expect(await cache.readCached(address)).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "persistent", error: "UnknownError" });
    expect(eventNames()).toContain("cache-resting");
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it("stands down after one round when it has never opened in this page, so the boot paint is not held", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const open = failOpens(Infinity, new DOMException("Unable to open database file on disk", "UnknownError"));

    expect(await cache.readCached(address)).toBeUndefined();
    expect(open).toHaveBeenCalledTimes(FAST_RECOVERY.reopenDelaysMs.length);
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "persistent" });
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-stood-down"]);
  });

  it("stands down when every transaction keeps failing on a database that opens", async () => {
    cache.setCacheRecoveryTiming({ ...FAST_RECOVERY, giveUpAfterMs: 100 });
    await cache.writeCached(address, { head: "stored" });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(() => {
      throw new DOMException("An internal error was encountered in the Indexed Database server", "UnknownError");
    });

    expect(await cache.readCached(address)).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "persistent" });
  });
});

describe("an open blocked by another tab", () => {
  /** Another tab running an older build holds version 2 open and does not
   *  answer `versionchange` on its own. */
  const olderTab = () => new Promise((resolve) => {
    const request = indexedDB.open("build-cache", 2);
    request.onupgradeneeded = () => request.result.createObjectStore("records");
    request.onsuccess = () => resolve(request.result);
  });

  it("waits for the other connection to close rather than standing down, and says it is waiting", async () => {
    const older = await olderTab();
    const open = vi.spyOn(indexedDB, "open");
    const write = cache.writeCached(address, { head: "after-upgrade" });

    await vi.waitFor(() => expect(eventNames()).toContain("cache-open-blocked"));
    // Past the open timeout: a blocked open is not a lost one.
    await new Promise((resolve) => setTimeout(resolve, FAST_RECOVERY.openTimeoutMs + 50));
    expect(cache.cacheHealth()).toMatchObject({ state: "blocked", since: expect.any(Number) });
    older.close();

    await write;
    expect((await cache.readCached(address))?.value).toEqual({ head: "after-upgrade" });
    expect(open).toHaveBeenCalledTimes(1);
    expect(eventNames()).toEqual(["cache-open-blocked"]);
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("stands down when the other tab never lets go, and answers what was waiting", async () => {
    cache.setCacheRecoveryTiming({ ...FAST_RECOVERY, blockedTimeoutMs: 100 });
    const older = await olderTab();
    vi.spyOn(console, "warn").mockImplementation(() => {});

    expect(await cache.readCached(address)).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "blocked" });
    expect(eventNames()).toEqual(["cache-open-blocked", "cache-stood-down"]);
    older.close();
  });

  it("steps aside when a newer version opens in another tab", async () => {
    await cache.writeCached(address, { head: "stored" });
    const newer = await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache", 4);
      request.onblocked = () => reject(new Error("the cache's connection blocked the newer version"));
      request.onsuccess = () => resolve(request.result);
    });
    newer.close();
    expect(eventNames()).toEqual(["cache-yielded"]);
  });
});

describe("what a reopen cannot fix", () => {
  it("stands down for a private window refusing IndexedDB", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const open = vi.spyOn(indexedDB, "open").mockImplementation(() =>
      failingOpenRequest(new DOMException("A mutation operation was attempted on a database that did not allow mutations.", "InvalidStateError")));
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed));

    expect(await cache.readCached(address)).toBeUndefined();
    await cache.writeCached(address, { head: "never" });

    expect(open).toHaveBeenCalledTimes(1);
    expect(heard).toEqual([]);
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "open-failed", error: "InvalidStateError" });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(eventNames()).toEqual(["cache-stood-down"]);
  });

  it("stands down for a browser that forbids storage outright", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(indexedDB, "open").mockImplementation(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });
    expect(await cache.readCached(address)).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", error: "SecurityError" });
  });

});

describe("a full quota", () => {
  it("fails the write that did not fit alone, and keeps answering reads", async () => {
    await cache.writeCached(address, { head: "stored" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
    });
    await cache.writeCached(address, { head: "too-big" });
    put.mockRestore();

    expect(heard).toEqual([]);
    expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
    expect(cache.cacheHealth()).toMatchObject({ state: "ready", lastRefused: { error: "QuotaExceededError", at: expect.any(Number) } });
    expect(eventNames()).toEqual(["cache-write-refused"]);
  });

  it("fails it alone when the browser aborts the transaction for quota", async () => {
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      const opened = originalTransaction.apply(this, args);
      if (args[1] === "readwrite") {
        Object.defineProperty(opened, "error", { get: () => new DOMException("The quota has been exceeded.", "QuotaExceededError") });
        queueMicrotask(() => opened.abort());
      }
      return opened;
    });
    await cache.writeCached(address, { head: "too-big" });
    transaction.mockRestore();

    expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
    expect(cache.cacheHealth().state).toBe("ready");
  });
});

describe("the record of it", () => {
  it("says a refused write was refused, and leaves the cache ready", async () => {
    await cache.writeCached(address, () => "a function cannot be stored");
    expect(eventNames()).toEqual(["cache-write-refused"]);
    expect(cacheEvents()[0]).toMatchObject({ error: "DataCloneError" });
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("says it has not been used until something has been read or written", async () => {
    expect(cache.cacheHealth()).toEqual({ state: "unused" });
    await cache.readCached(address);
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("says a read's attempts ran out once per outage, not once per round it waits through", async () => {
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failing = true;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (failing) throw connectionLost();
      return originalTransaction.apply(this, args);
    });
    const read = cache.readCached(address);
    await vi.waitFor(() => expect(transaction.mock.calls.length).toBeGreaterThan(3 * FAST_RECOVERY.reopenDelaysMs.length));
    failing = false;
    await read;
    expect(eventNames().filter((name) => name === "cache-operation-failed")).toHaveLength(1);
  });

  it("answers absent where there is no IndexedDB at all", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    expect(bare.cacheHealth()).toEqual({ state: "absent" });
  });
});
