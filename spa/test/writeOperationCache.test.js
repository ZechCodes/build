import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBIndex, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

const scope = { accountId: "account-a", deviceId: "device-a", principalId: "principal-a" };
const address = { deviceId: scope.deviceId, entityId: "project", kind: "task" };
const publication = [{ kind: "project", id: "project", sequence: 7, generation: 0 }];
let cache, operations, factory;
const intent = (operationId = "op-a", extra = {}) => ({
  operationId, revision: 0, request: { method: "tasks.comment", params: { body: "draft" } }, draft: "draft", ...extra,
});
const overlay = (value) => [{ address, kind: "merge", value }];

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  factory = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache;
  operations = factory(scope);
  operations.activateProjection();
});

describe("durable write operations", () => {
  it("persists normalized intent across reload, replica eviction and wipe", async () => {
    await operations.put(intent());
    await cache.writeCached(address, { body: "canonical" });
    await cache.evictEntity(scope.deviceId, address.entityId);
    await cache.wipeCache();
    vi.resetModules();
    const restored = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(scope);
    expect(await restored.get("op-a")).toMatchObject(intent());
  });

  it("isolates account, device and principal namespaces", async () => {
    await operations.put(intent());
    for (const changed of [{ accountId: "other" }, { deviceId: "other" }, { principalId: "other" }]) {
      expect(await factory({ ...scope, ...changed }).list()).toEqual([]);
    }
  });

  it("rejects a refused intent write and announces nothing", async () => {
    const heard = [];
    operations.subscribe((id) => heard.push(id));
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => { throw new DOMException("full", "QuotaExceededError"); });
    await expect(operations.put(intent())).rejects.toThrow();
    put.mockRestore();
    expect(await operations.list()).toEqual([]);
    expect(heard).toEqual([]);
  });

  it("serializes cross-tab updates without losing either change", async () => {
    await operations.put(intent());
    const other = factory(scope);
    await Promise.all([
      operations.update("op-a", (row) => ({ ...row, attempts: (row.attempts || 0) + 1 })),
      other.update("op-a", (row) => ({ ...row, attempts: (row.attempts || 0) + 1 })),
    ]);
    expect((await operations.get("op-a")).attempts).toBe(2);
  });

  it("retains the newest operation revision when ack arrives after confirmation", async () => {
    await operations.put(intent());
    const confirmation = { operation_id: "op-a", state: "succeeded", revision: 2, published: publication };
    await operations.confirm("op-a", { confirmation, revision: 2 });
    await operations.accept("op-a", { ack: { write: { revision: 1 } }, revision: 1, overlay: overlay({ body: "late" }) });
    expect(await operations.get("op-a")).toMatchObject({ revision: 2, confirmation });
    expect(await cache.readCached(address)).toBeUndefined();
  });
});

describe("effective cache projections", () => {
  it("composes accepted normalization in real selectors and keeps canonical mutations clean", async () => {
    await cache.writeCached(address, { body: "canonical", count: 0 });
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "normalized" }) });
    expect((await cache.readCached(address)).value.body).toBe("normalized");
    expect((await cache.readCachedMany([address]))[0].value.body).toBe("normalized");
    await cache.mergeCached(address, (value) => ({ ...value, count: 1 }));
    expect((await cache.readCanonicalCached(address)).value).toEqual({ body: "canonical", count: 1 });
    expect((await cache.readCached(address)).value).toEqual({ body: "normalized", count: 1 });
  });

  it("rebases later layers when only one unapplied operation fails", async () => {
    await cache.writeCached(address, { body: "base", title: "base" });
    await operations.put(intent());
    await operations.put(intent("op-b"));
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "a" }) });
    await operations.accept("op-b", { ack: {}, revision: 1, overlay: overlay({ title: "b" }) });
    await cache.writeCached(address, { body: "new base", title: "base" });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "failed", effect: "unapplied", error: { message: "refused" } } });
    expect((await cache.readCached(address)).value).toEqual({ body: "new base", title: "b" });
    expect(await operations.get("op-a")).toMatchObject({ draft: "draft", overlayActive: false, confirmation: { effect: "unapplied" } });
  });

  it("keeps partial and unknown outcomes as unresolved evidence", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "failed", effect: "partial" } });
    expect(await operations.get("op-a")).toMatchObject({ unresolved: true, overlayActive: true });
    expect((await cache.readCached(address)).value.body).toBe("accepted");
  });

  it("retains a successful overlay through invalidation and older canonical reads", async () => {
    await cache.writeCached(address, { body: "old" });
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    await cache.writeCached(address, { body: "stale pull" });
    expect((await cache.readCached(address)).value.body).toBe("accepted");
    await operations.observePublication({ canonical: [{ address, value: { body: "committed normalized" }, publication }] });
    expect((await cache.readCached(address)).value.body).toBe("committed normalized");
    expect((await operations.get("op-a")).overlayActive).toBe(false);
  });

  it("atomically publishes canonical records and removes accepted layers", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    const seen = [];
    cache.subscribeCache(address, () => seen.push(cache.readCached(address)));
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication }, canonical: [{ address, value: { body: "canonical" }, publication }] });
    expect((await Promise.all(seen)).every((record) => record.value.body === "canonical")).toBe(true);
  });

  it("composes upserts on a nested row list without duplicate reserved IDs", async () => {
    await cache.writeCached(address, { task: { comments: [{ id: "reserved", body: "old" }] } });
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: [{ address, kind: "upsert", path: ["task", "comments"], value: { id: "reserved", body: "new" }, key: "id" }] });
    expect((await cache.readCached(address)).value.task.comments).toEqual([{ id: "reserved", body: "new" }]);
  });

  it("excludes a previous account's accepted data after projection deactivation", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "private" }) });
    operations.deactivateProjection();
    expect(await cache.readCached(address)).toBeUndefined();
  });
});

describe("per-device transport leases", () => {
  it("admits one live holder across principals and transfers only after expiry or release", async () => {
    const other = factory({ ...scope, principalId: "other" });
    expect(await operations.acquireLease("tab-a", 10000)).toBe(true);
    expect(await other.acquireLease("tab-b", 10000)).toBe(false);
    expect(await other.renewLease("tab-b", 10000)).toBe(false);
    expect(await operations.renewLease("tab-a", 10000)).toBe(true);
    expect(await other.releaseLease("tab-b")).toBe(false);
    expect(await operations.releaseLease("tab-a")).toBe(true);
    expect(await other.acquireLease("tab-b", 10000)).toBe(true);
  });
});

describe("projection recovery and authority", () => {
  it("preserves an authoritative canonical upsert using a nested identity", async () => {
    const authoritative = { type: "thread.message", data: { operation_id: "op-a", body: "committed" } };
    await cache.writeCached(address, { items: [authoritative] });
    await operations.put(intent());
    await operations.accept("op-a", { revision: 1, ack: {}, overlay: [{ address, kind: "upsert", path: ["items"], key: ["data", "operation_id"], insertOnly: true, value: { data: { operation_id: "op-a", body: "provisional" } } }] });
    expect((await cache.readCached(address)).value.items).toEqual([authoritative]);
  });

  it("can reconcile an unresolved delivery with later definitive evidence", async () => {
    await operations.put(intent("op-a", { state: "outcome_unknown", unresolved: true }));
    await operations.accept("op-a", { revision: 1, ack: {}, overlay: overlay({ body: "saved" }) });
    expect(await operations.get("op-a")).toMatchObject({ state: "accepted", unresolved: false });
  });

  it("does not let an update change an operation identity", async () => {
    await operations.put(intent());
    await expect(operations.update("op-a", (row) => ({ ...row, operationId: "op-b" }))).rejects.toThrow();
    expect((await operations.get("op-a")).operationId).toBe("op-a");
  });

  it("allows an explicit account reset to clear durable records and projections", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { revision: 1, ack: {}, overlay: overlay({ body: "private" }) });
    const { wipePendingOperations } = await import("../src/core/writeOperationCache.js");
    await wipePendingOperations();
    expect(await operations.list()).toEqual([]);
    expect(await cache.readCached(address)).toBeUndefined();
  });
});

describe("publication refresh fences", () => {
  it("checks opening authority inside the publication transaction", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    await operations.observePublication({ accept: () => false, canonical: [{ address, value: { body: "old opening" }, publication }] });
    expect((await cache.readCached(address)).value.body).toBe("accepted");
    expect((await operations.get("op-a")).overlayActive).toBe(true);
  });

  it("keeps an intervening canonical push when a captured refresh returns", async () => {
    await cache.writeCached(address, { body: "before" });
    const snapshot = await cache.readCanonicalCached(address);
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    await cache.writeCached(address, { body: "new push" });
    await operations.observePublication({ canonical: [{ address, value: { body: "stale refresh" }, publication, guard: { written: cache.recordWriteOf(snapshot) } }] });
    expect((await cache.readCanonicalCached(address)).value.body).toBe("new push");
    expect((await operations.get("op-a")).overlayActive).toBe(true);
  });
});

describe("stable operation identity", () => {
  it("refuses a different request or dependency set under an existing ID", async () => {
    await operations.put(intent("op-a", { after: ["before-a"] }));
    await expect(operations.put(intent("op-a", { request: { method: "tasks.comment", params: { body: "different" } }, after: ["before-a"] }))).rejects.toThrow("operation_conflict");
    await expect(operations.put(intent("op-a", { after: ["before-b"] }))).rejects.toThrow("operation_conflict");
    expect((await operations.get("op-a")).request.params.body).toBe("draft");
  });

  it("reuses an identical intent without resetting a newer accepted state", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    expect(await operations.put(intent())).toMatchObject({ revision: 1, state: "accepted", acknowledged: true });
  });
});

describe("durability boundaries", () => {
  it("adds operation stores without discarding current-format replicas", async () => {
    await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache", 4);
      request.onupgradeneeded = () => request.result.createObjectStore("records").createIndex("at", "at");
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        const transaction = db.transaction("records", "readwrite");
        transaction.objectStore("records").put({ at: 1, value: { body: "v4 replica" } }, "device-a|project|task|");
        transaction.oncomplete = () => { db.close(); resolve(); };
      };
    });
    await operations.put(intent());
    expect((await cache.readCached(address)).value.body).toBe("v4 replica");
  });

  it("reclaims an expired tab lease atomically", async () => {
    await operations.acquireLease("tab-a", 1);
    await new Promise((resolve) => setTimeout(resolve, 5));
    const other = factory(scope);
    const results = await Promise.all([operations.acquireLease("tab-b", 10000), other.acquireLease("tab-c", 10000)]);
    expect(results.filter(Boolean)).toHaveLength(1);
    expect(await operations.renewLease("tab-a", 10000)).toBe(false);
  });

  it("announces committed operations to another tab without exposing row contents", async () => {
    const observed = [];
    const channel = new BroadcastChannel("build-write-operations");
    channel.onmessage = (event) => observed.push(event.data);
    await operations.put(intent());
    await vi.waitFor(() => expect(observed).toHaveLength(1));
    channel.close();
    expect(observed).toEqual([{ key: "account-a|device-a|principal-a|op-a" }]);
  });

  it("hears another tab's operation commit then reads the durable row", async () => {
    const heard = [];
    operations.subscribe((id) => heard.push(id));
    vi.resetModules();
    const another = (await import("../src/core/writeOperationCache.js")).createWriteOperationCache(scope);
    await another.put(intent());
    await vi.waitFor(() => expect(heard).toEqual(["op-a"]));
    expect(await operations.get("op-a")).toMatchObject(intent());
  });

  it("requires publication evidence for every affected address before retirement", async () => {
    const second = { ...address, kind: "timeline" };
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: [...overlay({ body: "accepted" }), { address: second, kind: "replace", value: ["accepted"] }] });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    await operations.observePublication({ canonical: [{ address, value: { body: "committed" }, publication }] });
    expect((await operations.get("op-a")).overlayActive).toBe(true);
    await operations.observePublication({ canonical: [{ address: second, value: ["committed"], publication }] });
    expect((await operations.get("op-a")).overlayActive).toBe(false);
  });
});


describe("typed protocol evidence", () => {
  it("retains raw acknowledgement and confirmation fixtures without reshaping wire fields", async () => {
    const fixture = (name) => JSON.parse(readFileSync(new URL(`../../fixtures/api/v1/writes/${name}.json`, import.meta.url), "utf8"));
    const ack = fixture("ack"), confirmation = fixture("confirmed-success");
    const id = ack.write.operation_id;
    await operations.put(intent(id));
    await operations.accept(id, { ack, revision: ack.write.revision, overlay: overlay({ body: ack.result.overlay[0].body }) });
    await operations.confirm(id, { confirmation, revision: confirmation.revision });
    const record = await operations.get(id);
    expect(record.ack).toEqual(ack);
    expect(record.confirmation).toEqual(confirmation);
    expect(record.owners).toEqual(ack.write.owners);
  });

  it("orders accepted edits by a shared owner beyond the first owner", async () => {
    await operations.put(intent("op-a", { createdAt: 1 }));
    await operations.put(intent("op-b", { createdAt: 2 }));
    await operations.accept("op-a", { ack: {}, revision: 1, owners: [{ kind: "task", id: "a", sequence: 1, generation: 0 }, { kind: "project", id: "shared", sequence: 2, generation: 0 }], overlay: overlay({ body: "later" }) });
    await operations.accept("op-b", { ack: {}, revision: 1, owners: [{ kind: "task", id: "b", sequence: 1, generation: 0 }, { kind: "project", id: "shared", sequence: 1, generation: 0 }], overlay: overlay({ body: "earlier" }) });
    expect((await cache.readCached(address)).value.body).toBe("later");
  });
});


describe("owner generation fences", () => {
  it("keeps an accepted layer when publication belongs to a replacement owner generation", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    const replacement = publication.map((fence) => ({ ...fence, generation: 1, sequence: 100 }));
    await operations.observePublication({ canonical: [{ address, value: { body: "replacement owner" }, publication: replacement }] });
    expect((await operations.get("op-a")).overlayActive).toBe(true);
    expect((await cache.readCached(address)).value.body).toBe("accepted");
  });

  it("orders different owner generations by local submission rather than owner sequence", async () => {
    await operations.put(intent("op-a", { createdAt: 1 }));
    await operations.put(intent("op-b", { createdAt: 2 }));
    await operations.accept("op-a", { ack: {}, revision: 1, owners: [{ kind: "project", id: "shared", generation: 0, sequence: 100 }], overlay: overlay({ body: "old generation" }) });
    await operations.accept("op-b", { ack: {}, revision: 1, owners: [{ kind: "project", id: "shared", generation: 1, sequence: 1 }], overlay: overlay({ body: "new generation" }) });
    expect((await cache.readCached(address)).value.body).toBe("new generation");
  });

  it.each([undefined, -1, Infinity, Number.MAX_SAFE_INTEGER + 1])("rejects invalid owner generation %s", async (generation) => {
    await operations.put(intent());
    await expect(operations.accept("op-a", { ack: {}, revision: 1, owners: [{ kind: "project", id: "shared", sequence: 1, generation }], overlay: overlay({ body: "invalid" }) })).rejects.toThrow();
    expect((await operations.get("op-a")).revision).toBe(0);
  });
});

describe("operation opening authority", () => {
  it("checks ack authority after the storage transaction was queued", async () => {
    await operations.put(intent());
    let current = true;
    const pending = operations.accept("op-a", { revision: 1, ack: {}, overlay: overlay({ body: "old opening" }), accept: () => current });
    current = false;
    await pending;
    expect((await operations.get("op-a")).revision).toBe(0);
  });

  it("checks confirmation authority without persisting callback functions", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { revision: 1, ack: {}, overlay: overlay({ body: "accepted" }), accept: () => true });
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "failed", effect: "unapplied" }, accept: () => false });
    expect((await operations.get("op-a")).state).toBe("accepted");
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "failed", effect: "unapplied" }, accept: () => true });
    expect((await operations.get("op-a")).state).toBe("failed");
    expect((await operations.get("op-a")).accept).toBeUndefined();
  });
});


describe("active-layer indexed reads", () => {
  it("renders only active layers in the addressed authenticated namespace", async () => {
    await operations.put(intent("active"));
    await operations.accept("active", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    await operations.put(intent("terminal", { state: "succeeded", rawHistory: "retained".repeat(1000) }));
    await operations.put(intent("unsent"));
    const unrelated = factory({ ...scope, accountId: "other-account" });
    await unrelated.put(intent("foreign"));
    await unrelated.accept("foreign", { ack: {}, revision: 1, overlay: overlay({ body: "foreign" }) });
    const loaded = [];
    let database;
    await cache.transactWriteOperations("readonly", ({ operations: store }) => { database = store.transaction.db; return null; });
    const original = IDBIndex.prototype.getAll;
    const indexed = vi.spyOn(IDBIndex.prototype, "getAll").mockImplementation(function (...args) {
      const read = original.apply(this, args);
      const index = this.name;
      if (this.objectStore.transaction.db === database) {
        read.addEventListener("success", () => loaded.push({ index, rows: read.result.map((row) => row.operationId) }));
      }
      return read;
    });
    try {
      expect((await cache.readCached(address)).value.body).toBe("accepted");
      expect(loaded).toEqual([{ index: "overlayNamespace", rows: ["active"] }]);
      loaded.length = 0;
      expect((await operations.list()).map((row) => row.operationId)).toEqual(["active", "terminal", "unsent"]);
      expect(loaded).toEqual([{ index: "namespace", rows: ["active", "terminal", "unsent"] }]);
    } finally { indexed.mockRestore(); }
  });

  it("removes retired layers from the sparse index", async () => {
    await operations.put(intent());
    await operations.accept("op-a", { ack: {}, revision: 1, overlay: overlay({ body: "accepted" }) });
    expect((await operations.get("op-a")).overlayNamespace).toBeDefined();
    await operations.confirm("op-a", { revision: 2, confirmation: { state: "succeeded", published: publication } });
    await operations.observePublication({ canonical: [{ address, value: { body: "committed" }, publication }] });
    expect((await operations.get("op-a")).overlayNamespace).toBeUndefined();
    expect((await cache.readCached(address)).value.body).toBe("committed");
  });
});

it("distinguishes inferred dependencies from an explicit empty dependency override", async () => {
  await operations.put(intent());
  await expect(operations.put(intent("op-a", { after: [] }))).rejects.toThrow("operation_conflict");
});
