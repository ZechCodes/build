// The owner's lifetime rules for what a workspace holds: everything but the
// feed row goes at once when the workspace is finished or deleted, and ages
// out 72 h after its last write once the workspace is only recent. The lists
// (devices, projects, workspaces, feed) are nobody's workspace data and are
// replaced, never expired.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const NOW = 1_800_000_000_000;
const HOUR = 60 * 60 * 1000;

let cache;
let lifetime;

/** A record written as if at a given moment: `at` is the TTL clock. */
const writeAt = async (address, value, at) => {
  const clock = vi.spyOn(Date, "now").mockReturnValue(at);
  await cache.writeCached(address, value);
  clock.mockRestore();
};

const held = async (address) => (await cache.readCached(address)) !== undefined;

const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  lifetime = await import("../src/core/cacheLifetime.js");
});

describe("the 72 h expiry", () => {
  it("is 72 h", () => {
    expect(lifetime.WORKSPACE_DATA_TTL_MS).toBe(72 * HOUR);
  });

  it("drops the workspace data last written more than 72 h ago, and keeps the rest", async () => {
    await writeAt(address("ws-1", "status"), { head: "old" }, NOW - 73 * HOUR);
    await writeAt(address("ws-1", "thread", "agent-1"), { items: [] }, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "tree", "src"), { entries: [] }, NOW - 72 * HOUR); // not yet older than
    await writeAt(address("ws-1", "log"), { commits: [] }, NOW - HOUR);
    await writeAt(address("ws-1", "row"), { kind: "workspace" }, NOW - 100 * HOUR);
    await writeAt(address("", "feed"), { items: [] }, NOW - 100 * HOUR);
    await writeAt(address("", "projects"), [], NOW - 100 * HOUR);

    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);

    expect(await held(address("ws-1", "status"))).toBe(false);
    expect(await held(address("ws-1", "thread", "agent-1"))).toBe(false);
    expect(await held(address("ws-1", "tree", "src"))).toBe(true);
    expect(await held(address("ws-1", "log"))).toBe(true);
    // The row belongs to the feed, which decides for itself what it lists.
    expect(await held(address("ws-1", "row"))).toBe(true);
    expect(await held(address("", "feed"))).toBe(true);
    expect(await held(address("", "projects"))).toBe(true);
  });

  it("leaves a neighbouring workspace's stale data alone", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW - 100 * HOUR);
    await writeAt(address("ws-2", "status"), {}, NOW - 100 * HOUR);
    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    expect(await held(address("ws-2", "status"))).toBe(true);
  });

  it("answers the addresses it dropped, and tells the surfaces holding them", async () => {
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1", entityId: "ws-1" }, (changed) => heard.push(changed.kind));
    await writeAt(address("ws-1", "status"), {}, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "log"), {}, NOW);
    const dropped = await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    expect(dropped.map((one) => one.kind)).toEqual(["status"]);
    expect(heard).toEqual(["status", "log", "status"]); // the two writes, then the drop
  });

  it("does nothing, and asks for nothing, when a workspace holds no stale data", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW);
    expect(await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW)).toEqual([]);
    expect(await held(address("ws-1", "status"))).toBe(true);
  });
});

describe("done and deleted", () => {
  it("drops every kind that workspace holds, however fresh, and leaves the row", async () => {
    const kinds = ["status", "log", "unpushed", "diff", "terminals", "console", "surfaces"];
    for (const kind of kinds) await writeAt(address("ws-1", kind), { kind }, NOW);
    await writeAt(address("ws-1", "patch", "abc123"), {}, NOW);
    await writeAt(address("ws-1", "file", "src/a.js"), { content: "a" }, NOW);
    await writeAt(address("ws-1", "row"), { kind: "workspace" }, NOW);

    await lifetime.evictWorkspaceData("dev-1", "ws-1");

    for (const kind of kinds) expect(await held(address("ws-1", kind))).toBe(false);
    expect(await held(address("ws-1", "patch", "abc123"))).toBe(false);
    expect(await held(address("ws-1", "file", "src/a.js"))).toBe(false);
    expect(await held(address("ws-1", "row"))).toBe(true);
  });

  it("takes nothing from a neighbour, or from the lists", async () => {
    await writeAt(address("ws-1", "status"), {}, NOW);
    await writeAt(address("ws-2", "status"), {}, NOW);
    await writeAt(address("ws-12", "status"), {}, NOW); // the id ws-1 is a prefix of
    await writeAt(address("", "workspaces"), [], NOW);
    await lifetime.evictWorkspaceData("dev-1", "ws-1");
    expect(await held(address("ws-2", "status"))).toBe(true);
    expect(await held(address("ws-12", "status"))).toBe(true);
    expect(await held(address("", "workspaces"))).toBe(true);
  });
});

describe("what a sweep reads", () => {
  /** Watch the ways a record body can leave the store. `getAll`, `get` and a
   *  value cursor each deserialize one; `getAllKeys` and an index key cursor
   *  do not. The spies sit on the store's prototype, so they see every
   *  transaction the module opens. */
  const watchBodyReads = async () => {
    await cache.readCached(address("ws-1", "status")); // the module opens its connection
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache");
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const prototype = Object.getPrototypeOf(db.transaction("records", "readonly").objectStore("records"));
    db.close();
    const read = [];
    const spies = ["get", "getAll", "openCursor"].map((method) => {
      const original = prototype[method];
      return vi.spyOn(prototype, method).mockImplementation(function spy(...args) {
        read.push(method);
        return original.apply(this, args);
      });
    });
    return { read, stop: () => spies.forEach((spy) => spy.mockRestore()) };
  };

  it("never deserializes a record body to decide what has aged out or must go", async () => {
    // A workspace holds up to five file bodies of 1 MB, a 256 KB working-tree
    // diff and twenty patches. A sweep that read them would structured-clone
    // tens of megabytes onto the main thread and throw all of it away — on
    // boot and on tab return, the two frames this plan exists to protect.
    await writeAt(address("ws-1", "file", "src/big.js"), { content: "x".repeat(64) }, NOW - 100 * HOUR);
    await writeAt(address("ws-1", "status"), {}, NOW);
    const watch = await watchBodyReads();

    await lifetime.expireWorkspaceData("dev-1", "ws-1", NOW);
    await lifetime.evictWorkspaceData("dev-1", "ws-1");

    watch.stop();
    expect(watch.read).toEqual([]);
    expect(await held(address("ws-1", "file", "src/big.js"))).toBe(false);
    expect(await held(address("ws-1", "status"))).toBe(false);
  });
});

describe("the recent files", () => {
  it("keeps 5", () => {
    expect(lifetime.RECENT_FILES).toBe(5);
  });

  it("keeps the five most recently opened and drops the rest", async () => {
    for (let index = 0; index < 7; index += 1) {
      await writeAt(
        address("ws-1", "file", `src/${index}.js`),
        { content: String(index), openedAt: NOW + index },
        NOW,
      );
    }
    await lifetime.trimRecentFiles("dev-1", "ws-1");
    const kept = await cache.cachedSubKeys("dev-1", "ws-1", "file");
    expect(kept.sort()).toEqual(["src/2.js", "src/3.js", "src/4.js", "src/5.js", "src/6.js"]);
  });

  it("reads a file with no opened-at as opened when it was written", async () => {
    await writeAt(address("ws-1", "file", "old.js"), { content: "old" }, NOW - HOUR);
    for (let index = 0; index < 5; index += 1) {
      await writeAt(address("ws-1", "file", `new-${index}.js`), { content: "n" }, NOW);
    }
    await lifetime.trimRecentFiles("dev-1", "ws-1");
    expect(await held(address("ws-1", "file", "old.js"))).toBe(false);
    expect(await held(address("ws-1", "file", "new-0.js"))).toBe(true);
  });

  it("leaves five or fewer alone, and touches no other kind", async () => {
    await writeAt(address("ws-1", "file", "a.js"), { content: "a" }, NOW);
    await writeAt(address("ws-1", "tree", "src"), { entries: [] }, NOW);
    expect(await lifetime.trimRecentFiles("dev-1", "ws-1")).toEqual([]);
    expect(await held(address("ws-1", "file", "a.js"))).toBe(true);
    expect(await held(address("ws-1", "tree", "src"))).toBe(true);
  });
});
