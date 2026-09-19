// The local cache: one IndexedDB store holding what the surfaces last saw, so
// a revisit paints from disk before the bridge answers. Plaintext by decision
// (2026-08-31): E2EE protects the wire; the browser profile is trusted.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const DB_NAME = "build-cache";
const STORE = "records";

let cache;

const freshFactory = () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
};

/** A database in an earlier format, holding one record, closed again — what a
 *  browser that ran a previous build has on disk. */
const seedVersion = (version, key, record) =>
  new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, version);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const transaction = db.transaction(STORE, "readwrite");
      transaction.objectStore(STORE).put(record, key);
      transaction.oncomplete = () => {
        db.close(); // an open v1 connection would block the upgrade
        resolve();
      };
    };
  });

beforeEach(async () => {
  vi.resetModules();
  freshFactory();
  cache = await import("../src/core/localCache.js");
});

describe("records", () => {
  it("round-trips a record and stamps when it was written", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(record.value).toEqual({ head: "abc" });
    expect(typeof record.at).toBe("number");
  });

  it("answers undefined for what was never written", async () => {
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-9", kind: "status" })).toBeUndefined();
  });

  it("keeps sub-keyed records apart", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, ["a.js"]);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "test" }, ["b.js"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" })).value).toEqual(["a.js"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "test" })).value).toEqual(["b.js"]);
  });

  it("does not let a separator in a sub key bleed into another record", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd|name" }, ["x"]);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd" })).toBeUndefined();
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd|name" })).value).toEqual(["x"]);
  });

  it("keeps two devices' records apart even for the same entity id", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "one" });
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" }, { head: "two" });
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("one");
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.head).toBe("two");
  });
});

describe("eviction", () => {
  it("evicts every record one entity holds and nothing anyone else does", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "abc123" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-12", kind: "status" }, { keep: true });
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" }, { keep: true });
    await cache.evictEntity("dev-1", "run-1");
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "abc123" })).toBeUndefined();
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-12", kind: "status" })).value.keep).toBe(true);
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.keep).toBe(true);
  });

  it("wipes everything on request", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.wipeCache();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
  });
});

describe("a browser without IndexedDB", () => {
  it("stays silent: reads answer undefined, writes and evictions do not throw", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    await expect(bare.writeCached({ deviceId: "d", entityId: "e", kind: "status" }, {})).resolves.toBeUndefined();
    // Nothing is held in memory: with no store there is no cache, and every
    // read answers "never seen" rather than this session's own writes.
    expect(await bare.readCached({ deviceId: "d", entityId: "e", kind: "status" })).toBeUndefined();
    expect(await bare.readCachedMany([{ deviceId: "d", entityId: "e", kind: "status" }])).toEqual([undefined]);
    await expect(bare.evictEntity("d", "e")).resolves.toBeUndefined();
  });

  it("announces nothing, because nothing changed", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    const heard = [];
    bare.subscribeCache({ deviceId: "d" }, (changed) => heard.push(changed));
    // A surface told its record changed re-reads it and finds nothing there,
    // and blanks what it was correctly painting a frame earlier.
    await bare.writeCached({ deviceId: "d", entityId: "e", kind: "status" }, { head: "abc" });
    await bare.deleteCached([{ deviceId: "d", entityId: "e", kind: "status" }]);
    await bare.evictEntity("d", "e");
    await bare.wipeCache();
    expect(heard).toEqual([]);
  });
});

describe("a write that fails", () => {
  it("says nothing changed, rather than sending a subscriber to re-read it", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed));
    // A value IndexedDB cannot store: the put throws inside the transaction,
    // and the cache stands down for the session.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, () => "not storable");
    expect(heard).toEqual([]);
    // Degraded to "no cache", silently — never to "what you hold has changed".
    await expect(cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).resolves.toBeUndefined();
  });
});

describe("the format version", () => {
  // v2 is as stale as v1 here: it has the records but not the write-time
  // index the lifetime sweeps walk, and a store missing an index a reader
  // asks for would stand the cache down for good.
  for (const version of [1, 2]) {
    it(`clears a database v${version} wrote, rather than reading its shapes`, async () => {
      vi.resetModules();
      freshFactory();
      await seedVersion(version, "dev-1|run-1|status|", { at: 1, value: { head: `from v${version}` } });
      const upgraded = await import("../src/core/localCache.js");
      expect(await upgraded.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
      // And the store is usable afterwards: a cold start, not a broken cache.
      await upgraded.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "now" });
      expect((await upgraded.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("now");
      // Including the sweeps, which need the index the old format lacks.
      expect(await upgraded.cachedAddressesWrittenBefore({ deviceId: "dev-1", entityId: "run-1" }, Date.now() + 1))
        .toEqual([{ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" }]);
    });
  }
});

describe("readCachedMany", () => {
  it("answers one record per address, in the order asked, undefined for what is missing", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, ["a.js"]);
    const records = await cache.readCachedMany([
      { deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" },
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { deviceId: "dev-1", entityId: "run-1", kind: "status" },
    ]);
    expect(records.map((record) => record?.value)).toEqual([["a.js"], undefined, { head: "abc" }]);
  });

  it("answers nothing for no addresses", async () => {
    expect(await cache.readCachedMany([])).toEqual([]);
  });
});

describe("announcements", () => {
  /** Resolve on the subscriber's first call, or reject if nothing arrives —
   *  the timer also keeps the loop alive while a channel message is in
   *  flight. */
  const announced = (address, { within = 2000 } = {}) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("nothing was announced")), within);
      cache.subscribeCache(address, (changed) => {
        clearTimeout(timer);
        resolve(changed);
      });
    });

  it("tells a subscriber on the entity which address was written", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    expect(await heard).toEqual({ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" });
  });

  it("announces only to the listeners the address is under", async () => {
    const seen = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(["device", changed.kind]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-1", kind: "tree" }, (changed) => seen.push(["tree", changed.sub]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, () => seen.push(["one-tree"]));
    cache.subscribeCache({ deviceId: "dev-2" }, () => seen.push(["other-device"]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-2" }, () => seen.push(["other-entity"]));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src-two" }, []);
    expect(seen).toEqual([["device", "tree"], ["tree", "src-two"]]);
  });

  it("announces an eviction to everyone holding any of that entity", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    await cache.evictEntity("dev-1", "run-1");
    expect(await heard).toEqual({ deviceId: "dev-1", entityId: "run-1" });
  });

  it("stops announcing once the subscriber lets go", async () => {
    const seen = [];
    const unsubscribe = cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(changed.kind));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    unsubscribe();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, {});
    expect(seen).toEqual(["status"]);
  });

  it("keeps one throwing subscriber from stopping the next one, or the write", async () => {
    const seen = [];
    cache.subscribeCache({ deviceId: "dev-1" }, () => {
      throw new Error("a view that is already unmounted");
    });
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(changed.kind));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    expect(seen).toEqual(["status"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("abc");
  });

  it("hears another tab's write over the channel and re-reads", async () => {
    // The other tab writes the record into the shared database, then says so.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "elsewhere" });
    // Only now does this tab listen. Subscribing before that write would let
    // its own local announcement satisfy both assertions, and the test would
    // pass with the inbound channel handler deleted.
    const heard = announced({ deviceId: "dev-1", entityId: "run-1" });
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.postMessage({ key: "dev-1|run-1|status|" });
    const changed = await heard;
    otherTab.close();
    expect(changed).toEqual({ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" });
    expect((await cache.readCached(changed)).value.head).toBe("elsewhere");
  });

  it("tells every subscriber, on every tab, that a wipe took the lot", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    const posted = [];
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.onmessage = (event) => posted.push(event.data);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.wipeCache();
    await heard;
    await new Promise((resolve) => setTimeout(resolve, 20));
    otherTab.close();
    expect(posted).toEqual([{ key: "dev-1|run-1|status|" }, { key: "" }]);
  });

  it("tells the other tabs what it wrote, and does not echo its own message back", async () => {
    const posted = [];
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.onmessage = (event) => posted.push(event.data);
    const echoed = [];
    cache.subscribeCache({ deviceId: "dev-1" }, () => echoed.push(1));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.evictEntity("dev-1", "run-1");
    await new Promise((resolve) => setTimeout(resolve, 20));
    otherTab.close();
    expect(posted).toEqual([{ key: "dev-1|run-1|status|" }, { key: "dev-1|run-1" }]);
    expect(echoed).toHaveLength(2); // the two local writes, not four
  });
});
