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
