// The cached-body fetcher: decide, one fetch, write through, answer. A body the
// local cache already holds is never asked for again, and a body the caller
// asks for a second time is refetched — that is how a stale one is replaced.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let createCachedBodies, cache;

const address = (key) => ({ deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: key });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ createCachedBodies } = await import("../src/core/cachedBodies.js"));
});

/** Bodies over a scripted wire: every fetch is recorded, and the answer is one
 *  item per key, in the config's own shape. */
const bodiesOver = (fetches, { addressOf = address } = {}) =>
  createCachedBodies({
    addressOf,
    fetchMissing: async (keys) => {
      fetches.push([...keys]);
      return keys.map((key) => ({ path: key, patch: `patch:${key}` }));
    },
    valueOf: (item) => ({ key: item.path, value: { patch: item.patch } }),
  });

describe("createCachedBodies", () => {
  it("holds nothing before it is asked", () => {
    const bodies = bodiesOver([]);
    expect(bodies.has("a.js")).toBe(false);
    expect(bodies.read("a.js")).toBeUndefined();
  });

  it("fetches what neither memory nor the local cache holds, and answers it", async () => {
    const fetches = [];
    const bodies = bodiesOver(fetches);
    await bodies.ensure(["a.js", "b.js"]);
    expect(fetches).toEqual([["a.js", "b.js"]]);
    expect(bodies.read("a.js")).toEqual({ patch: "patch:a.js" });
    expect(bodies.has("b.js")).toBe(true);
  });

  it("writes every fetched body through to the local cache", async () => {
    const bodies = bodiesOver([]);
    await bodies.ensure(["a.js"]);
    expect((await cache.readCached(address("a.js"))).value).toEqual({ patch: "patch:a.js" });
  });

  it("takes a body from the local cache instead of the wire", async () => {
    await cache.writeCached(address("a.js"), { patch: "from disk" });
    const fetches = [];
    const bodies = bodiesOver(fetches);
    await bodies.ensure(["a.js"]);
    expect(fetches).toEqual([]);
    expect(bodies.read("a.js")).toEqual({ patch: "from disk" });
  });

  it("fetches only what the local cache could not answer", async () => {
    await cache.writeCached(address("a.js"), { patch: "from disk" });
    const fetches = [];
    const bodies = bodiesOver(fetches);
    await bodies.ensure(["a.js", "b.js"]);
    expect(fetches).toEqual([["b.js"]]);
  });

  it("refetches a key the caller asks for again — the stale-body path", async () => {
    await cache.writeCached(address("a.js"), { patch: "from disk" });
    const fetches = [];
    const bodies = bodiesOver(fetches);
    await bodies.ensure(["a.js"]);
    await bodies.ensure(["a.js"]);
    expect(fetches).toEqual([["a.js"]]);
    expect(bodies.read("a.js")).toEqual({ patch: "patch:a.js" });
  });

  it("answers the keys it filled, so a caller can repaint only when something arrived", async () => {
    const bodies = bodiesOver([]);
    expect(await bodies.ensure(["a.js"])).toEqual(["a.js"]);
    expect(await bodies.ensure([])).toEqual([]);
  });

  it("asks for each key once when one is named twice", async () => {
    const fetches = [];
    const bodies = bodiesOver(fetches);
    await bodies.ensure(["a.js", "a.js"]);
    expect(fetches).toEqual([["a.js"]]);
  });

  it("still fetches where the surface has no cache address (a primary checkout)", async () => {
    const fetches = [];
    const bodies = bodiesOver(fetches, { addressOf: () => null });
    await bodies.ensure(["a.js"]);
    expect(fetches).toEqual([["a.js"]]);
    expect(bodies.read("a.js")).toEqual({ patch: "patch:a.js" });
  });

  it("lets a failing fetch throw, holding nothing for the keys it was asked for", async () => {
    const bodies = createCachedBodies({
      addressOf: address,
      fetchMissing: async () => {
        throw new Error("git.diff: path escapes the worktree");
      },
      valueOf: (item) => ({ key: item.path, value: item }),
    });
    await expect(bodies.ensure(["a.js"])).rejects.toThrow("escapes the worktree");
    expect(bodies.has("a.js")).toBe(false);
  });
});
