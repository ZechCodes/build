// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { writeFileSync } from "node:fs";
import { IDBFactory, IDBKeyRange, IDBDatabase, IDBObjectStore } from "fake-indexeddb";

const deviceId = "lineage-read-device";
const address = (entityId, kind) => ({ deviceId, entityId, kind });
const childRow = (agent = {}) => ({ kind: "branch", entity_id: "child-row", project_id: "p1", branch: "child", agents: [{ id: "child", created_by: "boss", working: true, ...agent }] });
let cache, reader, changed, pending, snapshots;

async function idle() {
  for (let turn = 0; turn < 3; turn += 1) {
    await vi.waitFor(() => expect(pending).toBe(0));
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  await cache.writeCached(address("", "agent-lineage-support"), { namesMakers: true });
  await cache.writeCached(address("child-row", "row"), childRow());
  pending = 0;
  const transaction = IDBDatabase.prototype.transaction;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    const result = transaction.apply(this, args);
    pending += 1;
    const finish = () => { pending -= 1; };
    result.addEventListener("complete", finish);
    result.addEventListener("abort", finish);
    return result;
  });
  snapshots = [];
  changed = vi.fn(() => snapshots.push(reader.buildAgentsFor("boss")?.map(({ id }) => id) || []));
  const { mountAgentLineage } = await import("../src/core/agentLineage.js");
  reader = mountAgentLineage({ deviceId, projectId: "p1", onChanged: changed });
  reader.start();
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(1));
  await idle();
  changed.mockClear();
  snapshots.length = 0;
});

afterEach(() => { reader.dispose(); vi.restoreAllMocks(); });

it("shares warmed support records across 32 no-op row hints", async () => {
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  for (let index = 0; index < 32; index += 1) {
    cache.announceCacheEvidence(address("child-row", "row"));
    await idle();
  }
  const reads = {};
  for (const [key] of gets.mock.calls) {
    const kind = decodeURIComponent(key.split("|")[2]);
    reads[kind] = (reads[kind] || 0) + 1;
  }
  if (process.env.LINEAGE_PROFILE_OUT) writeFileSync(process.env.LINEAGE_PROFILE_OUT, `${JSON.stringify({ hints: 32, reads, changed: changed.mock.calls.length }, null, 2)}\n`);
  expect(reads["agent-lineage-support"] || 0).toBe(0);
  expect(reads.row).toBe(32);
  expect(changed).not.toHaveBeenCalled();
});

it("reacts to support capability changes without a row rewrite", async () => {
  await cache.writeCached(address("", "agent-lineage-support"), { namesMakers: false });
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toBeNull());
  await cache.writeCached(address("", "agent-lineage-support"), { namesMakers: true });
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(1));
});

it.each(["entity", "device"])("clears an idle mounted lineage on %s prefix eviction", async (scope) => {
  if (scope === "entity") await cache.evictEntity(deviceId, "child-row");
  else await cache.wipeCache();
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toBeNull());
});

async function holdNextRowRead() {
  const readMany = cache.readCachedMany;
  let release;
  vi.spyOn(cache, "readCachedMany").mockImplementationOnce(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => { release = resolve; });
    return records;
  });
  await cache.writeCached(address("child-row", "row"), childRow({ working: false }));
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await idle();
  return release;
}

it("never combines pre-reset support with rows read after a held response", async () => {
  const release = await holdNextRowRead();
  try {
    await cache.wipeCache();
    await cache.writeCached(address("", "agent-lineage-support"), { namesMakers: false });
    await cache.writeCached(address("child-row", "row"), childRow({ id: "replacement" }));
  } finally {
    release();
  }
  await idle();
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toBeNull());
  expect(snapshots.every((ids) => ids.length === 0)).toBe(true);
});

it("does not restore an entity retired while its row response was held", async () => {
  const release = await holdNextRowRead();
  try {
    await cache.evictEntity(deviceId, "child-row");
  } finally {
    release();
  }
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toBeNull());
  await idle();
  expect(snapshots.every((ids) => ids.length === 0)).toBe(true);
});

it("does not publish a held response after disposal", async () => {
  const release = await holdNextRowRead();
  reader.dispose();
  release();
  await idle();
  expect(changed).not.toHaveBeenCalled();
});
