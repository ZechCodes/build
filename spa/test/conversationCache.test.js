// The conversation's records, without a rail around them: what a seed reports,
// what a re-read replaces, and what a message sent from here puts on the
// record before the wire has carried it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const identity = {
  deviceId: "dev-1",
  entityId: "run-3",
  agentId: "ag-1",
  surfaceSessionGeneration: "gen-1",
};
const threadAddress = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" };
const item = (sequence) => ({ type: "message", data: { sequence, role: "agent", body: `m-${sequence}` } });
const savedWindow = { items: [item(4)], deliveredSequence: 4 };
const shells = (description) => ({ shells: [{ id: "sh-1", description, state: "running" }] });

let cache, surfaces, conversationCache;
let seededThread, seededSurfaces, standing, threadCache;

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((done) => setTimeout(done, 0));
};

const stubThreadCache = () => ({
  seeded: null,
  seeds: 0,
  resets: 0,
  seedWindow(window) {
    this.seeds += 1;
    this.seeded = window;
    return !!window;
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

describe("opening a conversation from what is on disk", () => {
  it("reports both records to whoever is painting", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await saveSurfaces(shells("cargo test"));
    const held = mountCache();

    await held.seed();

    expect(threadCache.seeded).toEqual(savedWindow);
    expect(seededThread).toBe("ag-1");
    expect(seededSurfaces.surfaces).toEqual(shells("cargo test"));
  });

  it("names the record the panel is reading, so a watcher can hear it move", () => {
    expect(mountCache().address()).toEqual(threadAddress);
    standing = null;
    expect(mountCache().address()).toBe(null);
  });

  it("reads the records once per conversation", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    const held = mountCache();
    await held.seed();
    threadCache.seeded = null;

    await held.seed();
    expect(threadCache.seeded).toBe(null);
  });

  // The record IS the conversation: a write to it — a page the sync layer
  // pulled, a push it applied, a message this panel sent — is read back whole
  // rather than merged into what the panel was holding.
  it("opens the record again whenever it moves", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    const held = mountCache();
    await held.seed();

    const wider = { items: [item(1), item(4)], deliveredSequence: 4 };
    await cache.writeCached(threadAddress, wider);
    await held.reread();

    expect(threadCache.seeded).toEqual(wider);
    expect(seededThread).toBe("ag-1");
  });

  it("keeps a painted window when a cache reread finds no record", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    const held = mountCache();
    await held.seed();

    await cache.evictEntity("dev-1", "run-3");
    await held.reread();

    expect(threadCache.seeded).toEqual(savedWindow);
    expect(threadCache.seeds).toBe(1);
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

  it("drops a seed when the entity changes without changing the agent id", async () => {
    await saveSurfaces(shells("old entity"));
    const held = mountCache();
    const seeding = held.seed();
    standing = { ...identity, entityId: "run-4" };
    await seeding;
    expect(seededSurfaces).toBe(null);
  });
});

// A sent message is on the conversation the moment it is written, and the
// conversation is the record — so that is where it goes, keyed by the
// operation carrying it, and it leaves when the wire brings the real thing.
describe("a message sent from this panel", () => {
  const sentItems = async () => (await cache.readCached(threadAddress)).value.items;

  it("stands on the record until the post has been answered", async () => {
    await cache.writeCached(threadAddress, savedWindow);

    await conversationCache.writeProvisionalMessage(threadAddress, "op-1", { body: "ship it" });

    const items = await sentItems();
    expect(items.map((entry) => entry.data.body)).toEqual(["m-4", "ship it"]);
    expect(items[1].data.sequence).toBe(null);
    expect(items[1].data.delivery_status).toBe("queued");
  });

  it("opens a record for the first thing ever said in a conversation", async () => {
    await conversationCache.writeProvisionalMessage(threadAddress, "op-1", { body: "first words" });

    const record = (await cache.readCached(threadAddress)).value;
    expect(record.items.map((entry) => entry.data.body)).toEqual(["first words"]);
    expect(record.deliveredSequence).toBe(0);
  });

  it("takes the sequence the post was written at", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await conversationCache.writeProvisionalMessage(threadAddress, "op-1", { body: "ship it" });

    await conversationCache.acknowledgeProvisionalMessage(threadAddress, "op-1", 5, "sent");

    const items = await sentItems();
    expect(items[1].data.sequence).toBe(5);
    expect(items[1].data.delivery_status).toBe("sent");
  });

  it("comes back off the record when the post was refused", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    await conversationCache.writeProvisionalMessage(threadAddress, "op-1", { body: "ship it" });

    await conversationCache.withdrawProvisionalMessage(threadAddress, "op-1");

    expect((await sentItems()).map((entry) => entry.data.body)).toEqual(["m-4"]);
  });

  // Nothing waits on these writes: the send is already on its way and the
  // announcement the write makes is what paints. So a disk that will not take
  // one has to come back as a warning and stop there — a rejection nobody is
  // holding is an unhandled rejection at the top of an ordinary send.
  it("warns rather than throws at a send when the disk will not take the write", async () => {
    vi.resetModules();
    vi.doMock("../src/core/localCache.js", async () => ({
      ...(await vi.importActual("../src/core/localCache.js")),
      mergeCached: () => Promise.reject(new Error("the cache is gone")),
    }));
    const writes = await import("../src/core/conversationCache.js");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

    await expect(writes.writeProvisionalMessage(threadAddress, "op-1", { body: "ship it" })).resolves.toBeUndefined();
    await expect(writes.acknowledgeProvisionalMessage(threadAddress, "op-1", 5, "sent")).resolves.toBeUndefined();
    await expect(writes.withdrawProvisionalMessage(threadAddress, "op-1")).resolves.toBeUndefined();

    expect(warn).toHaveBeenCalledTimes(3);
    warn.mockRestore();
    vi.doUnmock("../src/core/localCache.js");
  });

  it("leaves the record alone when there is nothing of that operation on it", async () => {
    await cache.writeCached(threadAddress, savedWindow);
    const before = await cache.readCached(threadAddress);

    await conversationCache.withdrawProvisionalMessage(threadAddress, "op-nothing");
    await conversationCache.acknowledgeProvisionalMessage(threadAddress, "op-nothing", 9, "sent");

    expect((await cache.readCached(threadAddress)).at).toBe(before.at);
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

  it("keeps a conversation-keyed transcript under the workspace, keyed by the conversation", () => {
    standing = conversational;
    expect(mountCache().address()).toEqual(conversationAddress);
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

  it("settles a message sent to it whatever the panel has moved on to", async () => {
    // The address is taken from the submission, not from what is on screen: a
    // reader who presses send and walks to another bubble has still sent it.
    await cache.writeCached(threadAddress, savedWindow);
    await conversationCache.writeProvisionalMessage(threadAddress, "op-1", { body: "ship it" });
    standing = { ...identity, agentId: "ag-2" };

    await conversationCache.acknowledgeProvisionalMessage(threadAddress, "op-1", 5, "sent");
    await settle();

    expect((await cache.readCached(threadAddress)).value.items[1].data.sequence).toBe(5);
  });
});
