// #206: unsent drafts and UI state live in their own database, versioned apart
// from the replica cache. A replica schema bump, a restart, a sweep or an
// eviction never takes a draft, and the `ui-*` records a build before the
// split wrote into the replica store are carried across without losing any —
// including the ones an older tab, still open, keeps writing there.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const DRAFT = { deviceId: "dev-1", entityId: "conv-1", kind: "ui-draft", sub: "chat:main" };
const FOLD = { deviceId: "", entityId: "", kind: "ui-fold", sub: "inbox:projects" };
const REPLICA = { deviceId: "dev-1", entityId: "conv-1", kind: "status" };

let cache;
let store;

const freshFactory = () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
};

/** A new page: every module evaluated again over the same browser storage. */
async function reload() {
  vi.resetModules();
  cache = await import("../src/core/localCache.js");
  store = await import("../src/core/localUiStore.js");
}

/** The raw records of one database's `records` store, keyed as stored. */
const rawRecords = (name) => new Promise((resolve, reject) => {
  const request = indexedDB.open(name);
  request.onerror = () => reject(request.error);
  request.onsuccess = () => {
    const db = request.result;
    if (!db.objectStoreNames.contains("records")) {
      db.close();
      resolve({});
      return;
    }
    const held = {};
    const cursor = db.transaction("records").objectStore("records").openCursor();
    cursor.onsuccess = () => {
      if (!cursor.result) {
        db.close();
        resolve(held);
        return;
      }
      held[cursor.result.key] = cursor.result.value;
      cursor.result.continue();
    };
  };
});

const DRAFT_KEY = "dev-1|conv-1|ui-draft|chat%3Amain";

/** A browser that ran a build before the split: build-cache at `version`,
 *  its replica store holding a draft beside a replica record. */
const seedOldCache = (version, records) => new Promise((resolve, reject) => {
  const request = indexedDB.open("build-cache", version);
  request.onupgradeneeded = () => request.result.createObjectStore("records").createIndex("at", "at");
  request.onerror = () => reject(request.error);
  request.onsuccess = () => {
    const db = request.result;
    const transaction = db.transaction("records", "readwrite");
    for (const [key, record] of Object.entries(records)) transaction.objectStore("records").put(record, key);
    transaction.oncomplete = () => {
      db.close();
      resolve();
    };
  };
});

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.spyOn(console, "info").mockImplementation(() => {});
  freshFactory();
  await reload();
});

describe("the local UI store", () => {
  it("keeps what it is given in its own database, not the replica cache", async () => {
    await store.writeUiRecord(DRAFT, { body: "half a thought" });
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "half a thought" });
    expect(await cache.readCached(DRAFT)).toBeUndefined();
    expect(Object.keys(await rawRecords("build-ui"))).toEqual([DRAFT_KEY]);
    expect(await rawRecords("build-cache")).toEqual({});
  });

  it("keeps a draft across a restart", async () => {
    await store.writeUiRecord(DRAFT, { body: "before the reload" });
    await reload();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "before the reload" });
  });

  it("announces a write to whoever watches the address", async () => {
    const heard = [];
    store.subscribeUiRecords(DRAFT, (address) => heard.push(address.kind));
    await store.writeUiRecord(DRAFT, { body: "typed" });
    expect(heard).toEqual(["ui-draft"]);
  });

  it("replays a page-exit edit only while it is the newest", async () => {
    await store.writeUiRecord(DRAFT, { body: "committed" }, { source: "tab-a", sequence: 4 });
    expect(await store.writeUiRecordIfNewer(DRAFT, { body: "older" }, { at: 1, source: "tab-b", sequence: 1 })).toBe(false);
    expect(await store.writeUiRecordIfNewer(DRAFT, { body: "later" }, { at: Date.now() + 1000, source: "tab-b", sequence: 1 }))
      .toBe(true);
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "later" });
  });

  it("forgets everything on an account reset", async () => {
    await store.writeUiRecord(DRAFT, { body: "the last account's" });
    await store.wipeUiRecords();
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
  });

  it("keeps every draft when a build with an older version of it opens", async () => {
    // A rollback, or a stale tab: this store at v2, then a build that knows
    // only v1. That build stands down; it never drops what is here.
    await store.writeUiRecord(DRAFT, { body: "written before the rollback" });
    await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-ui", 2);
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        request.result.close();
        resolve();
      };
    });
    await reload();
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
    await store.writeUiRecord(FOLD, { open: true });
    expect(Object.keys(await rawRecords("build-ui"))).toEqual([DRAFT_KEY]);
    expect((await rawRecords("build-ui"))[DRAFT_KEY].value).toEqual({ body: "written before the rollback" });
  });
});

describe("replica lifetime never reaches a draft", () => {
  it("survives evicting the entity it is filed under", async () => {
    await store.writeUiRecord(DRAFT, { body: "keep me" });
    await cache.writeCached(REPLICA, { head: "abc" });
    await cache.evictEntity("dev-1", "conv-1");
    expect(await cache.readCached(REPLICA)).toBeUndefined();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "keep me" });
  });

  it("survives the workspace sweeps", async () => {
    const lifetime = await import("../src/core/cacheLifetime.js");
    await store.writeUiRecord(DRAFT, { body: "keep me" });
    await cache.writeCached(REPLICA, { head: "abc" });
    await lifetime.evictWorkspaceData("dev-1", "conv-1");
    await lifetime.expireWorkspaceData("dev-1", "conv-1", Date.now() + 365 * 24 * 3600 * 1000);
    expect(await cache.readCached(REPLICA)).toBeUndefined();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "keep me" });
  });

  it("is not evicted or swept from the replica store before it is carried", async () => {
    const lifetime = await import("../src/core/cacheLifetime.js");
    // A draft an older tab wrote where that build keeps drafts, beside a
    // replica record of the same entity.
    await cache.writeCached(DRAFT, { body: "not carried yet" });
    await cache.writeCached(REPLICA, { head: "abc" });
    await cache.evictEntity("dev-1", "conv-1");
    await lifetime.evictWorkspaceData("dev-1", "conv-1");
    await lifetime.expireWorkspaceData("dev-1", "conv-1", Date.now() + 365 * 24 * 3600 * 1000);
    await cache.deleteCached([DRAFT]);
    expect(Object.keys(await rawRecords("build-cache"))).toEqual([DRAFT_KEY]);
    await store.adoptCachedUiRecords();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "not carried yet" });
  });

  it("survives the replica database being dropped outright", async () => {
    // What a tab on an older build does when it meets a newer replica
    // version: VersionError, then deleteDatabase("build-cache").
    await store.writeUiRecord(DRAFT, { body: "keep me" });
    await cache.writeCached(REPLICA, { head: "abc" });
    await cache.wipeCache();
    await new Promise((resolve) => {
      const request = indexedDB.deleteDatabase("build-cache");
      request.onsuccess = resolve;
      request.onblocked = resolve;
    });
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "keep me" });
  });

  it("survives a replica schema bump", async () => {
    await store.writeUiRecord(DRAFT, { body: "keep me" });
    await cache.writeCached(REPLICA, { head: "abc" });
    // The next replica version, whatever it rebuilds: here, everything.
    await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache", 99);
      request.onupgradeneeded = () => {
        for (const name of [...request.result.objectStoreNames]) request.result.deleteObjectStore(name);
      };
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        request.result.close();
        resolve();
      };
    });
    await reload();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "keep me" });
  });
});

describe("drafts a build before the split left in the replica store", () => {
  const oldDraft = (body, at = 1000) => ({ at, order: at, write: `old:${at}`, value: { body } });

  it("are carried into the UI store and out of the replica store", async () => {
    freshFactory();
    await seedOldCache(4, {
      [DRAFT_KEY]: oldDraft("from v4"),
      "dev-1|conv-1|status|": { at: 1000, value: { head: "abc" } },
    });
    await reload();
    await store.adoptCachedUiRecords();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "from v4" });
    expect(Object.keys(await rawRecords("build-cache"))).toEqual(["dev-1|conv-1|status|"]);
  });

  it("are carried across on first use, and the watcher hears them arrive", async () => {
    freshFactory();
    await seedOldCache(4, { [DRAFT_KEY]: oldDraft("from v4") });
    await reload();
    const heard = [];
    store.subscribeUiRecords(DRAFT, () => heard.push("draft"));
    await store.readUiRecord(FOLD);
    await vi.waitFor(async () => expect((await store.readUiRecord(DRAFT))?.value).toEqual({ body: "from v4" }));
    expect(heard).toContain("draft");
  });

  it("never overwrite a newer draft already in the UI store", async () => {
    await store.writeUiRecord(DRAFT, { body: "newer, typed here" });
    await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache", 4);
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const transaction = request.result.transaction("records", "readwrite");
        transaction.objectStore("records").put(oldDraft("older, from v4"), DRAFT_KEY);
        transaction.oncomplete = () => {
          request.result.close();
          resolve();
        };
      };
    });
    await store.adoptCachedUiRecords();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "newer, typed here" });
    expect(await rawRecords("build-cache")).toEqual({});
  });

  it("survive a replica upgrade from an older format before they are carried", async () => {
    freshFactory();
    await seedOldCache(3, {
      [DRAFT_KEY]: oldDraft("from v3"),
      "dev-1|conv-1|status|": { at: 1000, value: { head: "abc" } },
    });
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    // The replica upgrade runs first, on its own: the draft must still be
    // there for the UI store to take.
    expect(await cache.readCached(REPLICA)).toBeUndefined();
    expect(Object.keys(await rawRecords("build-cache"))).toEqual([DRAFT_KEY]);
    store = await import("../src/core/localUiStore.js");
    await store.adoptCachedUiRecords();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "from v3" });
  });

  it("are carried across as an older tab, still open, keeps writing them", async () => {
    await store.readUiRecord(FOLD); // this tab is up and has adopted what there was
    // A tab still running the build before the split writes its draft
    // where that build keeps drafts: the replica store, announced on the
    // replica channel.
    vi.resetModules();
    const olderTab = await import("../src/core/localCache.js");
    await olderTab.writeCached(DRAFT, { body: "typed in the older tab" });
    await vi.waitFor(async () => expect((await store.readUiRecord(DRAFT))?.value).toEqual({ body: "typed in the older tab" }));
    await vi.waitFor(async () => expect(await rawRecords("build-cache")).toEqual({}));
  });

  it("are not carried into the next account by a pass running through a reset", async () => {
    freshFactory();
    await seedOldCache(4, { [DRAFT_KEY]: oldDraft("the last account's") });
    // Hold the pass between reading the replica store and writing here.
    let read;
    const hasRead = new Promise((resolve) => { read = resolve; });
    let release;
    const released = new Promise((resolve) => { release = resolve; });
    vi.resetModules();
    vi.doMock("../src/core/localCache.js", async (importOriginal) => {
      const actual = await importOriginal();
      return {
        ...actual,
        cachedUiRecords: async () => {
          const held = await actual.cachedUiRecords();
          read();
          await released;
          return held;
        },
      };
    });
    try {
      cache = await import("../src/core/localCache.js");
      store = await import("../src/core/localUiStore.js");
    } finally {
      vi.doUnmock("../src/core/localCache.js");
    }
    const pass = store.adoptCachedUiRecords();
    await hasRead;
    await Promise.all([cache.wipeCache(), store.wipeUiRecords()]);
    release();
    await pass;
    await store.adoptCachedUiRecords();
    expect(await rawRecords("build-ui")).toEqual({});
    expect(await rawRecords("build-cache")).toEqual({});
  });

  it("do not hold a mount's first read past its bound while the replica is away", async () => {
    freshFactory();
    await seedOldCache(3, { [DRAFT_KEY]: oldDraft("from v3") });
    // An older tab holds the replica at v3 and never lets the upgrade through.
    const holder = await new Promise((resolve) => {
      const request = indexedDB.open("build-cache", 3);
      request.onsuccess = () => resolve(request.result);
    });
    await reload();
    const heard = [];
    store.subscribeUiRecords(DRAFT, () => heard.push("draft"));
    const started = Date.now();
    expect(await store.readUiRecord(DRAFT)).toBeUndefined();
    const waited = Date.now() - started;
    expect(waited).toBeGreaterThanOrEqual(1400);
    expect(waited).toBeLessThan(4000);
    // The older tab goes; the draft is carried and announced.
    holder.close();
    await vi.waitFor(async () => expect((await store.readUiRecord(DRAFT))?.value).toEqual({ body: "from v3" }));
    expect(heard).toContain("draft");
  });

  it("leave a replica-store draft rewritten mid-carry for the next pass", async () => {
    await cache.writeCached(DRAFT, { body: "first" });
    const [carried] = await cache.cachedUiRecords();
    // Another tab rewrites the draft between the read and the removal.
    await cache.writeCached(DRAFT, { body: "rewritten" });
    await cache.deleteCachedIfUnwritten([carried]);
    expect((await cache.readCached(DRAFT)).value).toEqual({ body: "rewritten" });
    await store.adoptCachedUiRecords();
    expect((await store.readUiRecord(DRAFT)).value).toEqual({ body: "rewritten" });
    expect(await rawRecords("build-cache")).toEqual({});
  });
});
