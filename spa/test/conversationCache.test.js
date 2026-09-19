// The conversation's two saved records, without a rail around them: what a
// seed reports, what a write costs, and what a switch forgets.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const identity = {
  deviceId: "dev-1",
  entityId: "run-3",
  agentId: "ag-1",
  surfaceSessionGeneration: "gen-1",
};
const threadAddress = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" };
const savedWindow = { items: [{ id: "m-1" }], deliveredSequence: 4 };
const shells = (description) => ({ shells: [{ id: "sh-1", description, state: "running" }] });

let cache, surfaces, conversationCache;
let seededThread, seededSurfaces, standing, threadCache;

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((done) => setTimeout(done, 0));
};

const stubThreadCache = () => ({
  seeded: null,
  window: null,
  resets: 0,
  refuses: false,
  seedWindow(window) {
    if (this.refuses) return false;
    this.seeded = window;
    return true;
  },
  readWindow() {
    return this.window;
  },
  reset() {
    this.resets += 1;
  },
});

const mountCache = () =>
  conversationCache.createConversationCache({
    addressOf: () => standing,
    threadCache,
    onThreadSeeded: (agentId) => {
      seededThread = agentId;
    },
    onSurfacesSeeded: (seen) => {
      seededSurfaces = seen;
    },
  });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  surfaces = await import("../src/core/surfacesCache.js");
  conversationCache = await import("../src/core/conversationCache.js");
  seededThread = null;
  seededSurfaces = null;
  standing = identity;
  threadCache = stubThreadCache();
});

const saveSurfaces = (snapshot) =>
  cache.writeCached(surfaces.surfacesCacheAddress(identity), surfaces.surfacesRecord(snapshot, "gen-1"));

describe("seeding a conversation from what was saved", () => {
  it("reports both records to whoever is painting", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await saveSurfaces(shells("cargo test"));
    const held = mountCache();

    await held.seed();

    expect(threadCache.seeded).toEqual(savedWindow);
    expect(seededThread).toBe("ag-1");
    expect(seededSurfaces.surfaces).toEqual(shells("cargo test"));
  });

  it("reads the records once per conversation", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    const held = mountCache();
    await held.seed();
    threadCache.seeded = null;

    await held.seed();
    expect(threadCache.seeded).toBe(null);
  });

  it("reports nothing for the agent the reader left while the read was in flight", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await saveSurfaces(shells("cargo test"));
    const held = mountCache();

    const seeding = held.seed();
    standing = { ...identity, agentId: "ag-2" };
    await seeding;

    expect(seededThread).toBe(null);
    expect(seededSurfaces).toBe(null);
  });

  it("reports nothing at all while the entity or the device is unknown", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    standing = null;
    const held = mountCache();

    await held.seed();
    expect(seededThread).toBe(null);

    standing = identity;
    await held.seed();
    expect(seededThread).toBe("ag-1");
  });

  it("reports no surfaces for a record it cannot read", async () => {
    await cache.writeCached(surfaces.surfacesCacheAddress(identity), { surfaces: "boom" });
    const held = mountCache();

    await held.seed();
    expect(seededSurfaces).toBe(null);
  });

  it("reports no surfaces once a payload has answered for the agent", async () => {
    await saveSurfaces(shells("cargo test"));
    const held = mountCache();

    held.absorbSurfaces(shells("cargo clippy"), "gen-1");
    await held.seed();
    expect(seededSurfaces).toBe(null);
  });

  it("leaves the window alone when the cache refuses the seed", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    threadCache.refuses = true;
    const held = mountCache();

    await held.seed();
    expect(seededThread).toBe(null);
  });
});

describe("writing a conversation back through", () => {
  it("persists a window that moved, and rewrites nothing while it stands still", async () => {
    const held = mountCache();
    threadCache.window = savedWindow;

    held.persistThread();
    await settle();
    expect((await cache.readCached(threadAddress)).value).toEqual(savedWindow);

    await cache.wipeCache();
    held.persistThread();
    await settle();
    expect(await cache.readCached(threadAddress)).toBeUndefined();
  });

  it("persists the snapshot a payload moved, and rewrites nothing while it stands still", async () => {
    const held = mountCache();
    const address = surfaces.surfacesCacheAddress(identity);

    held.absorbSurfaces(shells("cargo test"), "gen-1");
    await settle();
    expect((await cache.readCached(address)).value).toEqual(surfaces.surfacesRecord(shells("cargo test"), "gen-1"));

    await cache.wipeCache();
    held.absorbSurfaces(shells("cargo test"), "gen-1");
    await settle();
    expect(await cache.readCached(address)).toBeUndefined();

    held.absorbSurfaces(shells("cargo clippy"), "gen-1");
    await settle();
    expect((await cache.readCached(address)).value.surfaces).toEqual(shells("cargo clippy"));
  });

  it("persists an answer carrying no surfaces as a whole-snapshot clear", async () => {
    await saveSurfaces(shells("from the last visit"));
    const held = mountCache();

    held.absorbSurfaces(null, "gen-1");
    await settle();
    expect((await cache.readCached(surfaces.surfacesCacheAddress(identity))).value).toEqual(
      surfaces.surfacesRecord(null, "gen-1"),
    );
  });

  it("does not seed a legacy or replaced process snapshot", async () => {
    const address = surfaces.surfacesCacheAddress(identity);
    await cache.writeCached(address, surfaces.surfacesRecord(shells("old"), "gen-old"));
    const held = mountCache();
    await held.seed();
    expect(seededSurfaces).toBe(null);
  });

  it("drops a seed when the process generation changes during its async read", async () => {
    await saveSurfaces(shells("old"));
    const held = mountCache();
    const seeding = held.seed();
    standing = { ...identity, surfaceSessionGeneration: "gen-2" };
    await seeding;
    expect(seededSurfaces).toBe(null);
  });

  it("drops a write when the agent or generation changes during its async guard read", async () => {
    const held = mountCache();
    held.absorbSurfaces(shells("old"), "gen-1");
    standing = { ...identity, surfaceSessionGeneration: "gen-2" };
    await settle();
    expect(await cache.readCached(surfaces.surfacesCacheAddress(identity))).toBeUndefined();
  });

  it("lets only the newest same-generation snapshot survive overlapping writes", async () => {
    const held = mountCache();
    held.absorbSurfaces(shells("older"), "gen-1");
    held.absorbSurfaces(shells("newest"), "gen-1");
    await settle();
    expect((await cache.readCached(surfaces.surfacesCacheAddress(identity))).value).toEqual(
      surfaces.surfacesRecord(shells("newest"), "gen-1"),
    );
  });

  it("drops a seed when the entity changes without changing the agent id", async () => {
    await saveSurfaces(shells("old entity"));
    const held = mountCache();
    const seeding = held.seed();
    standing = { ...identity, entityId: "run-4" };
    await seeding;
    expect(seededSurfaces).toBe(null);
  });
});

// The Records table (plan stage 4) addresses a transcript `ws | thread | agent
// or conversation id`: the workspace is the entity, the conversation id is the
// sub-key. Addressing it the other way round — the conversation id AS the
// entity — put the transcript outside the workspace's prefix, where
// `evictWorkspaceData` and the 72 h expiry could never reach it, so a Done or
// Deleted workspace left its conversations on disk for ever.
describe("where a conversation's transcript is stored", () => {
  const conversational = { ...identity, conversationId: "conv-9" };
  const conversationAddress = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "conv-9" };

  it("keeps a conversation-keyed transcript under the workspace, keyed by the conversation", async () => {
    standing = conversational;
    const held = mountCache();
    threadCache.window = savedWindow;

    held.persistThread();
    await settle();

    expect((await cache.readCached(conversationAddress)).value).toEqual(savedWindow);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "conv-9", kind: "thread", sub: "" })).toBeUndefined();
  });

  it("seeds that transcript back from under the workspace", async () => {
    await cache.writeCached(conversationAddress, savedWindow);
    standing = conversational;
    const held = mountCache();

    await held.seed();

    expect(threadCache.seeded).toEqual(savedWindow);
  });

  it("leaves the whole transcript inside the workspace's eviction prefix", async () => {
    await cache.writeCached(conversationAddress, savedWindow);
    await cache.evictEntity("dev-1", "run-3");
    expect(await cache.readCached(conversationAddress)).toBeUndefined();
  });
});

describe("the conversation the reader switched away from", () => {
  it("forgets the seed, the window and what was on disk", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await saveSurfaces(shells("cargo test"));
    const held = mountCache();
    await held.seed();
    seededThread = null;

    held.reset();
    expect(threadCache.resets).toBe(1);

    await held.seed();
    expect(seededThread).toBe("ag-1");
  });
});
