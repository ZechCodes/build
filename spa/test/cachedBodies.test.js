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
const bodiesOver = (fetches, { addressOf = address, onChange } = {}) =>
  createCachedBodies({
    addressOf,
    fetchMissing: async (keys) => {
      fetches.push([...keys]);
      return keys.map((key) => ({ path: key, patch: `patch:${key}` }));
    },
    valueOf: (item) => ({ key: item.path, value: { patch: item.patch } }),
    onChange,
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

  it("re-reads an announced cache write and reports that stored body as the change", async () => {
    const changes = [];
    let bodies;
    bodies = bodiesOver([], { onChange: (key) => changes.push([key, bodies.read(key)]) });
    await bodies.ensure(["a.js"]);

    await cache.writeCached(address("a.js"), { patch: "from another writer" });
    await vi.waitFor(() => expect(changes).toContainEqual(["a.js", { patch: "from another writer" }]));

    expect(bodies.read("a.js")).toEqual({ patch: "from another writer" });
    bodies.dispose();
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

describe("createCachedBodies with pages (#95)", () => {
  const lines = (count) => Array.from({ length: count }, (_, index) => `+line ${index}\n`).join("");
  const WHOLE = lines(400);

  /** A wire whose whole answer is cut at `cutAt` characters, and whose pages
   *  are cut from the whole patch by offset, named by `version()`. */
  const pagedOver = (fetches, pageReads, { canPage = true, cutAt = 1000, version = () => "v1" } = {}) =>
    import("../src/core/bodyPages.js").then((pages) =>
      createCachedBodies({
        addressOf: address,
        fetchMissing: async (keys) => {
          fetches.push([...keys]);
          return keys.map((key) => ({ path: key, content_key: "k1", patch: WHOLE.slice(0, cutAt), truncated: true }));
        },
        valueOf: (item) => ({ key: item.path, value: { content_key: item.content_key, patch: item.patch, truncated: item.truncated } }),
        cacheable: (value) => !value.truncated,
        pages: {
          field: "patch",
          split: (value, of) => pages.textPagesOf(value.patch, { of, cut: value.truncated, bytes: 512 }),
          readPage: async (_key, offset) => {
            pageReads.push(offset);
            if (!canPage) return null;
            return pages.textPagesOf(WHOLE, { of: version(), bytes: 512 }).find((page) => page.offset === offset) || null;
          },
        },
      }));

  it("keeps a body it may not store whole as a head and the bridge's own pages, and paints it from them", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const reads = [];
    const bodies = await pagedOver([], reads);
    await bodies.ensure(["big.txt"]);

    expect(reads).toEqual([0]);
    const head = await cache.readCached(address("big.txt"));
    expect(head.value).toEqual({ content_key: "k1", truncated: true, paged: true, of: "v1" });
    const held = bodies.read("big.txt");
    const first = pages.textPagesOf(WHOLE, { of: "v1", bytes: 512 })[0];
    expect(held.patch).toBe(first.body);
    expect(held.pages).toEqual({ end: first.end, total: WHOLE.length, complete: false });
    bodies.dispose();
  });

  it("splits the answer it has into pages when the bridge cannot page", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const bodies = await pagedOver([], [], { canPage: false });
    await bodies.ensure(["big.txt"]);
    const head = await cache.readCached(address("big.txt"));
    expect(head.value.of).toBe("whole");
    const held = bodies.read("big.txt");
    expect(held.patch).toBe(WHOLE.slice(0, WHOLE.lastIndexOf("\n", 999) + 1));
    expect(held.pages).toEqual({ end: held.patch.length, total: null, complete: false });
    expect(await bodies.more("big.txt")).toBe(false);
    bodies.dispose();
  });

  it("reads the next page from where the held pages end, into the cache, to the end", async () => {
    const reads = [];
    const bodies = await pagedOver([], reads);
    await bodies.ensure(["big.txt"]);
    const before = bodies.read("big.txt").pages.end;
    expect(await bodies.more("big.txt")).toBe(true);
    expect(reads).toEqual([0, before]);
    while (!bodies.read("big.txt").pages.complete) expect(await bodies.more("big.txt")).toBe(true);
    expect(bodies.read("big.txt").patch).toBe(WHOLE);
    expect(await bodies.more("big.txt")).toBe(false);
    bodies.dispose();
  });

  it("paints the pages another mount left, without the wire", async () => {
    const first = await pagedOver([], []);
    await first.ensure(["big.txt"]);
    await first.more("big.txt");
    const heldBefore = first.read("big.txt");
    first.dispose();

    const fetches = [];
    const second = await pagedOver(fetches, []);
    await second.ensure(["big.txt"]);
    expect(fetches).toEqual([]);
    expect(second.read("big.txt")).toEqual(heldBefore);
    second.dispose();
  });

  it("fetches the body again when a page says the patch moved, though its content key did not", async () => {
    let version = "v1";
    const fetches = [];
    const bodies = await pagedOver(fetches, [], { version: () => version });
    await bodies.ensure(["big.txt"]);
    version = "v2";
    expect(await bodies.more("big.txt")).toBe(false);
    expect(fetches).toEqual([["big.txt"], ["big.txt"]]);
    expect((await cache.readCached(address("big.txt"))).value.of).toBe("v2");
    bodies.dispose();
  });

  it("keeps no page for a head that was replaced while the page was read", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const all = pages.textPagesOf(WHOLE, { of: "v1", bytes: 512 });
    const bodies = createCachedBodies({
      addressOf: address,
      fetchMissing: async (keys) => keys.map((key) => ({ path: key, patch: WHOLE.slice(0, 1000), truncated: true })),
      valueOf: (item) => ({ key: item.path, value: { patch: item.patch, truncated: item.truncated } }),
      cacheable: (value) => !value.truncated,
      pages: {
        field: "patch",
        split: () => [],
        readPage: async (_key, offset) => {
          if (offset) await cache.writeCached(address("big.txt"), { patch: "someone else's", paged: false });
          return all.find((page) => page.offset === offset);
        },
      },
    });
    await bodies.ensure(["big.txt"]);
    expect(await bodies.more("big.txt")).toBe(false);
    expect(await cache.cachedSubKeys("dev-1", "run-1", pages.PAGE_RECORD_KIND)).toEqual(["filediff:big.txt@0"]);
    bodies.dispose();
  });

  it("lets the pages go when the body fits one record again", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const bodies = await pagedOver([], []);
    await bodies.ensure(["big.txt"]);
    bodies.dispose();
    const direct = createCachedBodies({
      addressOf: address,
      fetchMissing: async (keys) => keys.map((key) => ({ path: key, patch: "small" })),
      valueOf: (item) => ({ key: item.path, value: { content_key: "k3", patch: item.patch } }),
      pages: { field: "patch", split: () => [] },
    });
    await direct.ensure(["big.txt"]);
    await direct.ensure(["big.txt"]);
    expect(direct.read("big.txt")).toEqual({ content_key: "k3", patch: "small" });
    expect(await cache.cachedSubKeys("dev-1", "run-1", pages.PAGE_RECORD_KIND)).toEqual([]);
    direct.dispose();
  });
});
