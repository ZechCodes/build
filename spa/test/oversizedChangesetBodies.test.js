// A changeset file's body too large for one record, or cut by the bridge, is
// kept in pages (#95): painted from the cache like any other body, and read on
// a page at a time from a bridge that can cut one.

import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { pagedAnswer } from "./gitWireFixture.js";

let cache, pages, createChangesetBodies, cap;
const address = (path) => ({ deviceId: "dev-1", entityId: "run-1", kind: "changesetdiff", sub: path });
const patch = (path, text) => `diff --git a/${path} b/${path}\n@@ -1 +1 @@\n+${text}\n`;
const allOpen = (views) => new Set(views.map((view) => view.path));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  pages = await import("../src/core/bodyPages.js");
  ({ createChangesetBodies, CHANGESET_DIFF_MAX_BYTES: cap } = await import("../src/core/changesetBodies.js"));
});

it("keeps a normal changeset patch whole and an oversized one in pages, both painted from the cache", async () => {
  const fetchFiles = vi.fn(async (paths) => ({
    files: paths.map((path) => ({ path, content_key: "current" })),
    patch: paths.map((path) => patch(path, path === "large.txt" ? "x".repeat(cap + 1) : "small")).join(""),
  }));
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => "current", fetchFiles });
  const views = ["small.txt", "large.txt"].map((path) => ({ path, contentKey: "current" }));
  await bodies.sync(views, allOpen(views));

  expect((await cache.readCached(address("small.txt"))).value.patch).toContain("small");
  const head = (await cache.readCached(address("large.txt"))).value;
  expect(head).toMatchObject({ content_key: "current", paged: true, of: "whole" });
  expect(head.patch).toBeUndefined();
  expect((await pages.readBodyPages(address("large.txt"), "whole")).complete).toBe(true);
  expect(bodies.bodyOf("large.txt").patch).toContain("x".repeat(cap));
  bodies.dispose();

  const again = createChangesetBodies({ addressOf: address, keyFor: () => "current", fetchFiles: vi.fn() });
  await again.sync(views, allOpen(views));
  expect(again.bodyOf("large.txt").patch).toContain("x".repeat(cap));
  again.dispose();
  expect(fetchFiles).toHaveBeenCalledTimes(1);
});

const B_WHOLE = "diff --git a/b.txt b/b.txt\n@@ -1 +1,3 @@\n+half\n+and the rest\n+of it\n";
const cutAnswer = {
  truncated: true,
  files: [{ path: "a.txt", content_key: "ka" }, { path: "b.txt", content_key: "kb" }],
  patch: `${patch("a.txt", "whole")}${B_WHOLE.slice(0, 45)}`,
};
/** A bridge answering the cut aggregate, and pages of b.txt by range. */
const pagingBridge = (calls) =>
  vi.fn(async (paths, { range } = {}) => {
    calls.push({ paths, range });
    if (!range) return cutAnswer;
    return { ...pagedAnswer(B_WHOLE, range.offset, { version: "v-b", pageBytes: 30 }), files: [{ path: "b.txt", content_key: "kb" }] };
  });

it("leaves unreached aggregate paths uncached until their own wire bodies and page heads land", async () => {
  const paths = ["a.txt", "b.txt", "c.txt", "d.txt"];
  const files = paths.map((path) => ({ path, content_key: `k:${path}` }));
  const fetchFiles = vi.fn(async (wanted, { range } = {}) => {
    const text = wanted.map((path) => patch(path, "own body")).join("");
    if (range) return { ...pagedAnswer(text, range.offset, { version: `v:${wanted[0]}`, pageBytes: 30 }), files };
    return { files, patch: text, truncated: true };
  });
  const bodies = createChangesetBodies({ addressOf: address, keyFor: (path) => `k:${path}`, fetchFiles, canPage: () => true });
  await bodies.seed({ files, patch: patch("a.txt", "whole") + patch("b.txt", "cut"), truncated: true }, paths);

  expect(await cache.readCached(address("c.txt"))).toBeUndefined();
  expect(await cache.readCached(address("d.txt"))).toBeUndefined();
  const views = files.map((file) => ({ path: file.path, contentKey: file.content_key }));
  await bodies.sync(views, allOpen(views));
  await bodies.sync(views, allOpen(views));

  expect(fetchFiles.mock.calls.filter(([, options]) => !options?.range)).toEqual([[["c.txt", "d.txt"]]]);
  expect(bodies.bodyOf("c.txt").patch).toContain("+own body");
  expect((await cache.readCached(address("d.txt"))).value).toMatchObject({ paged: true, of: "v:d.txt" });
  while (await bodies.more("d.txt"));
  expect(bodies.bodyOf("d.txt")).toMatchObject({ patch: patch("d.txt", "own body"), pages: { complete: true } });
  bodies.dispose();
});

it("refetches an empty incomplete body left in the cache by an older truncated aggregate", async () => {
  const path = "unreached.txt";
  const currentKey = "current";
  const recoveredPatch = patch(path, "recovered body");
  await pages.writeBodyPages(address(path), pages.textPagesOf("", { of: "whole", cut: true }));
  await cache.writeCached(address(path), { content_key: currentKey, paged: true, of: "whole", truncated: true });
  expect(await pages.readBodyPages(address(path), "whole")).toMatchObject({ end: 0, total: null, complete: false });
  const fetchFiles = vi.fn(async () => ({ files: [{ path, content_key: currentKey }], patch: recoveredPatch }));
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => currentKey, fetchFiles });
  const views = [{ path, contentKey: currentKey }];
  try {
    await bodies.sync(views, allOpen(views));
    await bodies.sync(views, allOpen(views));

    expect(fetchFiles).toHaveBeenCalledExactlyOnceWith([path]);
    expect(bodies.bodyOf(path)).toMatchObject({ content_key: currentKey, patch: recoveredPatch });
    expect((await cache.readCached(address(path))).value.paged).toBeUndefined();
  } finally {
    bodies.dispose();
  }
});

it("keeps an empty complete paged body without refetching its truncated head", async () => {
  const path = "empty.txt";
  const currentKey = "current";
  await pages.writeBodyPages(address(path), pages.textPagesOf("", { of: "v-empty", cut: false }));
  await cache.writeCached(address(path), { content_key: currentKey, paged: true, of: "v-empty", truncated: true });
  const fetchFiles = vi.fn(async () => ({ files: [], patch: "" }));
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => currentKey, fetchFiles });
  const views = [{ path, contentKey: currentKey }];
  try {
    await bodies.sync(views, allOpen(views));
    await bodies.sync(views, allOpen(views));

    expect(fetchFiles).not.toHaveBeenCalled();
    expect(bodies.bodyOf(path)).toMatchObject({ patch: "", pages: { end: 0, total: 0, complete: true } });
  } finally {
    bodies.dispose();
  }
});

it("retries a rejected oversized fill and keeps the recovered page head", async () => {
  const fetchFiles = vi.fn().mockRejectedValueOnce(new Error("transport hiccup"))
    .mockImplementation(async (_paths, { range } = {}) => range
      ? { ...pagedAnswer(B_WHOLE, range.offset, { version: "v-b", pageBytes: 30 }), files: cutAnswer.files }
      : cutAnswer);
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => "kb", fetchFiles, canPage: () => true });
  const views = [{ path: "b.txt", contentKey: "kb" }];
  await expect(bodies.sync(views, allOpen(views))).rejects.toThrow("transport hiccup");
  expect(await cache.readCached(address("b.txt"))).toBeUndefined();
  await bodies.sync(views, allOpen(views));
  await bodies.sync(views, allOpen(views));

  expect(fetchFiles).toHaveBeenCalledTimes(3);
  expect((await cache.readCached(address("b.txt"))).value).toMatchObject({ paged: true, of: "v-b" });
  bodies.dispose();
});

it("marks only the file the bridge's cut fell in, and keeps it in the bridge's own pages", async () => {
  const calls = [];
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => undefined, fetchFiles: pagingBridge(calls), canPage: () => true });
  const views = [{ path: "a.txt", contentKey: "ka" }, { path: "b.txt", contentKey: "kb" }];
  await bodies.sync(views, allOpen(views));

  expect((await cache.readCached(address("a.txt"))).value).toMatchObject({ patch: expect.stringContaining("+whole") });
  expect(bodies.bodyOf("a.txt").truncated).toBeUndefined();
  expect((await cache.readCached(address("b.txt"))).value).toMatchObject({ paged: true, of: "v-b", truncated: true });
  expect(bodies.bodyOf("b.txt").pages).toMatchObject({ total: B_WHOLE.length, complete: false });

  while (await bodies.more("b.txt"));
  expect(bodies.bodyOf("b.txt")).toMatchObject({ patch: B_WHOLE, pages: { complete: true } });
  expect(calls.filter((call) => call.range).every((call) => call.paths.length === 1 && call.range.bytes === pages.BODY_PAGE_BYTES)).toBe(true);
  bodies.dispose();
});

// Pages split here out of an answer — a bridge that could not page when the
// body was kept — are "whole"; the bridge's pages are of a version. The first
// page read by range therefore starts the body over from the bridge's own
// first page, once, and reads on from there.
it("starts a body split before its bridge could page over from the bridge's pages, once", async () => {
  const calls = [];
  let paging = false;
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => undefined, fetchFiles: pagingBridge(calls), canPage: () => paging });
  const views = [{ path: "b.txt", contentKey: "kb" }];
  await bodies.sync(views, allOpen(views));
  expect((await cache.readCached(address("b.txt"))).value.of).toBe("whole");
  expect(await bodies.more("b.txt")).toBe(false);

  paging = true;
  expect(await bodies.more("b.txt")).toBe(false); // another version: the body is fetched again
  expect((await cache.readCached(address("b.txt"))).value.of).toBe("v-b");
  const refetches = calls.filter((call) => !call.range).length;
  while (await bodies.more("b.txt"));
  expect(bodies.bodyOf("b.txt")).toMatchObject({ patch: B_WHOLE, pages: { complete: true } });
  expect(calls.filter((call) => !call.range)).toHaveLength(refetches);
  expect(refetches).toBe(2);
  bodies.dispose();
});

it("never sends a range to a bridge that cannot page", async () => {
  const fetchFiles = vi.fn(async () => ({
    truncated: true,
    files: [{ path: "b.txt", content_key: "kb" }],
    patch: "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n+hal",
  }));
  const bodies = createChangesetBodies({ addressOf: address, keyFor: () => undefined, fetchFiles });
  const views = [{ path: "b.txt", contentKey: "kb" }];
  await bodies.sync(views, allOpen(views));
  expect(bodies.canPage()).toBe(false);
  expect(await bodies.more("b.txt")).toBe(false);
  expect(fetchFiles).toHaveBeenCalledTimes(1);
  expect(fetchFiles.mock.calls[0]).toEqual([["b.txt"]]);
  expect(bodies.bodyOf("b.txt")).toMatchObject({ truncated: true, pages: { complete: false } });
  bodies.dispose();
});
