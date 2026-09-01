// The local cache: one IndexedDB store holding what the surfaces last saw, so
// a revisit paints from disk before the bridge answers. Plaintext by decision
// (2026-08-31): E2EE protects the wire; the browser profile is trusted.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
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
    expect(await bare.readCached({ deviceId: "d", entityId: "e", kind: "status" })).toBeUndefined();
    await expect(bare.evictEntity("d", "e")).resolves.toBeUndefined();
  });
});
