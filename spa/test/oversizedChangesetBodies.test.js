import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, createChangesetBodies, cap;
const address = (path) => ({ deviceId: "dev-1", entityId: "run-1", kind: "changesetdiff", sub: path });
const patch = (path, text) => `diff --git a/${path} b/${path}\n@@ -1 +1 @@\n+${text}\n`;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ createChangesetBodies, CHANGESET_DIFF_MAX_BYTES: cap } = await import("../src/core/changesetBodies.js"));
});

it("keeps a normal changeset patch and paints an oversized one without a cache record", async () => {
  const bodies = createChangesetBodies({
    addressOf: address,
    keyFor: () => "current",
    fetchFiles: async (paths) => ({
      files: paths.map((path) => ({ path, content_key: "current" })),
      patch: paths.map((path) => patch(path, path === "large.txt" ? "x".repeat(cap + 1) : "small")).join(""),
    }),
  });
  const views = ["small.txt", "large.txt"].map((path) => ({ path, contentKey: "current" }));
  await bodies.sync(views, new Set(views.map((view) => view.path)));

  expect((await cache.readCached(address("small.txt"))).value.patch).toContain("small");
  expect(await cache.readCached(address("large.txt"))).toBeUndefined();
  expect(bodies.bodyOf("large.txt").patch).toContain("x".repeat(100));
  bodies.dispose();
});
