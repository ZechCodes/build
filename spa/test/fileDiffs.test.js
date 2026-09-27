// Per-file diffs: the status shape says which file changed and what its content
// is; the body of each file is fetched on its own, capped per call, cached
// under its path, and answered by path.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { pagedAnswer } from "./gitWireFixture.js";

let fileDiffs, cache, pages;

const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
};

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n`;

const statusFile = (path, contentKey) => ({
  path,
  staged: "none",
  index_status: "-",
  worktree_status: "M",
  content_key: contentKey,
  added: 1,
  deleted: 1,
  binary: false,
});

const status = (files) => ({
  branch: "main",
  files,
  files_truncated: false,
  stat: { files_changed: files.length, insertions: files.length, deletions: files.length },
  status_key: "aaaaaaaaaaaaaaaa",
});

const TWO = status([statusFile("a.js", "key-a"), statusFile("b.js", "key-b")]);

const bodyFor = (path, contentKey) => ({ content_key: contentKey, patch: patchFor(path, contentKey), truncated: false });

const address = (path) => ({ deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: path });

/** A git.diff channel that answers each requested path from a content map. */
const wireOver = (contentKeys, calls) =>
  vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method !== "git.diff") throw new Error(`unexpected ${method}`);
    return { files: params.paths.map((path) => ({ path, ...bodyFor(path, contentKeys[path]) })) };
  });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  fileDiffs = await import("../src/core/fileDiffs.js");
  pages = await import("../src/core/bodyPages.js");
});

const mountDiffs = (call) =>
  fileDiffs.createFileDiffs({ deviceId: "dev-1", entityId: "run-1", scope: { run_id: "run-1" }, call });

describe("pathsToFetch", () => {
  it("fetches an open file whose body is missing", () => {
    const wanted = fileDiffs.pathsToFetch(TWO, { openPaths: new Set(["a.js"]), cached: () => undefined });
    expect(wanted).toEqual(["a.js"]);
  });

  it("leaves a collapsed file alone — it is fetched on expand or by the warmer", () => {
    expect(fileDiffs.pathsToFetch(TWO, { openPaths: new Set(), cached: () => undefined })).toEqual([]);
  });

  it("fetches an open file whose content key moved, and leaves an unchanged one alone", () => {
    const cached = (path) => bodyFor(path, path === "a.js" ? "stale" : "key-b");
    const open = new Set(["a.js", "b.js"]);
    expect(fileDiffs.pathsToFetch(TWO, { openPaths: open, cached })).toEqual(["a.js"]);
  });

  it("wants nothing from a shape with no files", () => {
    expect(fileDiffs.pathsToFetch(status([]), { openPaths: new Set(["a.js"]) })).toEqual([]);
  });
});

describe("batchPaths", () => {
  it("keeps one call under the verb's cap and answers in request order", () => {
    const paths = Array.from({ length: 120 }, (_unused, index) => `f${index}.js`);
    const batches = fileDiffs.batchPaths(paths, fileDiffs.GIT_DIFF_MAX_PATHS);
    expect(batches.map((batch) => batch.length)).toEqual([50, 50, 20]);
    expect(batches.flat()).toEqual(paths);
  });

  it("makes no call for nothing to fetch", () => {
    expect(fileDiffs.batchPaths([], 50)).toEqual([]);
  });
});

describe("createFileDiffs", () => {
  it("fetches the open files' bodies and answers them by path", async () => {
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a", "b.js": "key-b" }, calls));
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    expect(calls).toEqual([{ method: "git.diff", params: { run_id: "run-1", paths: ["a.js"] } }]);
    expect(diffs.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    expect(diffs.bodyOf("b.js")).toBeUndefined();
    diffs.dispose();
  });

  it("writes each body through to the local cache under its path", async () => {
    const diffs = mountDiffs(wireOver({ "a.js": "key-a" }, []));
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    expect((await cache.readCached(address("a.js"))).value).toEqual(bodyFor("a.js", "key-a"));
    diffs.dispose();
  });

  it("keeps an oversized body in pages under its head and paints it from the cache", async () => {
    const large = `${patchFor("b.js", "key-b")}${"+x\n".repeat(400_000)}`;
    const call = vi.fn(async () => ({ files: [
      { path: "a.js", content_key: "key-a", patch: "ok", truncated: false },
      { path: "b.js", content_key: "key-b", patch: large, truncated: false },
    ] }));
    const diffs = mountDiffs(call);
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js", "b.js"]) });
    expect((await cache.readCached(address("a.js"))).value.patch).toBe("ok");
    const head = (await cache.readCached(address("b.js"))).value;
    expect(head).toMatchObject({ content_key: "key-b", paged: true, of: "whole" });
    expect(head.patch).toBeUndefined();
    const held = await pages.readBodyPages(address("b.js"), "whole");
    expect(held.pages.length).toBeGreaterThan(1);
    expect(held.complete).toBe(true);
    expect(diffs.bodyOf("b.js")).toMatchObject({ patch: large, pages: { complete: true } });
    diffs.dispose();

    // A second mount paints the same body off the disk, with no wire call.
    const again = vi.fn();
    const remount = mountDiffs(again);
    await remount.sync({ status: TWO, openPaths: new Set(["b.js"]) });
    expect(again).not.toHaveBeenCalled();
    expect(remount.bodyOf("b.js").patch).toBe(large);
    remount.dispose();
  });

  // A bridge that pages names its pages by a digest of the whole patch, so the
  // cache keeps its pages from the first — a content key does not name a patch.
  const WHOLE = "diff --git a/a.js b/a.js\n@@ -1,4 +1,4 @@\n+first line\n+second line\n+third line\n";
  const pagingDiff = (calls, { withRange = true } = {}) =>
    vi.fn(async (method, params) => {
      calls.push(params);
      if (!params.range) return { files: [{ path: "a.js", content_key: "next", patch: WHOLE.slice(0, 50), truncated: true }] };
      const page = pagedAnswer(WHOLE, params.range.offset, { version: "v-whole", pageBytes: 30 });
      return { files: [{ path: "a.js", content_key: "next", patch: page.patch, ...(withRange ? { range: page.range } : {}) }] };
    });
  const pagingDiffs = (call) =>
    fileDiffs.createFileDiffs({ deviceId: "dev-1", entityId: "run-1", scope: { run_id: "run-1" }, call, canPage: () => true });

  it("keeps a cut body in the bridge's own pages, named by its version, and reads the rest a page at a time", async () => {
    const calls = [];
    const diffs = pagingDiffs(pagingDiff(calls));
    await diffs.sync({ status: status([statusFile("a.js", "next")]), openPaths: new Set(["a.js"]) });
    expect((await cache.readCached(address("a.js"))).value).toMatchObject({ paged: true, of: "v-whole" });
    expect(diffs.bodyOf("a.js").pages).toMatchObject({ total: WHOLE.length, complete: false });

    while (await diffs.more("a.js"));
    expect(diffs.bodyOf("a.js")).toMatchObject({ patch: WHOLE, pages: { complete: true } });
    const ranged = calls.filter((params) => params.range);
    expect(ranged[0]).toEqual({ run_id: "run-1", paths: ["a.js"], range: { offset: 0, bytes: pages.BODY_PAGE_BYTES } });
    expect(ranged.map((params) => params.range.offset)).toEqual([...new Set(ranged.map((params) => params.range.offset))]);
    diffs.dispose();
  });

  it("takes a ranged answer that carries no range for no page at all", async () => {
    const calls = [];
    const diffs = pagingDiffs(pagingDiff(calls, { withRange: false }));
    await diffs.sync({ status: status([statusFile("a.js", "next")]), openPaths: new Set(["a.js"]) });
    const cutLines = WHOLE.slice(0, WHOLE.lastIndexOf("\n", 50) + 1);
    expect((await cache.readCached(address("a.js"))).value).toMatchObject({ paged: true, of: "whole" });
    expect(diffs.bodyOf("a.js")).toMatchObject({ patch: cutLines, pages: { total: null, complete: false } });
    expect(await diffs.more("a.js")).toBe(false);
    expect(diffs.bodyOf("a.js").patch).toBe(cutLines);
    diffs.dispose();
  });

  it("pages only for a bridge whose greeting announced bodies.pages", async () => {
    const events = await import("../src/core/changeEvents.js");
    const diffs = mountDiffs(vi.fn());
    expect(diffs.canPage()).toBe(false);
    await events.greetBridge(async () => ({ api_version: "2.0.0", capabilities: ["bodies.pages"] }), { deviceId: "dev-1" });
    expect(diffs.canPage()).toBe(true);
    events.resetChangeEvents();
    diffs.dispose();
  });

  it("reads a body back from the local cache instead of the wire", async () => {
    await cache.writeCached(address("a.js"), bodyFor("a.js", "key-a"));
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a" }, calls));
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    expect(calls).toEqual([]);
    expect(diffs.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    diffs.dispose();
  });

  it("replaces a cached body the shape says is stale", async () => {
    await cache.writeCached(address("a.js"), bodyFor("a.js", "older"));
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a" }, calls));
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    expect(calls).toHaveLength(1);
    expect(diffs.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    diffs.dispose();
  });

  it("takes a newer cache announcement instead of a late body pull", async () => {
    const answer = deferred();
    const call = vi.fn(() => answer.promise);
    const onChange = vi.fn();
    const diffs = fileDiffs.createFileDiffs({
      deviceId: "dev-1",
      entityId: "run-1",
      scope: { run_id: "run-1" },
      call,
      onChange,
    });
    const pending = diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));

    const newer = { content_key: "key-a", patch: patchFor("a.js", "newer cache write"), truncated: false };
    await cache.writeCached(address("a.js"), newer);
    answer.resolve({ files: [{ path: "a.js", ...bodyFor("a.js", "key-a"), patch: patchFor("a.js", "late pull") }] });
    await pending;
    await vi.waitFor(() => expect(onChange).toHaveBeenCalled());

    expect(diffs.bodyOf("a.js")).toEqual(newer);
    expect((await cache.readCached(address("a.js"))).value).toEqual(newer);
    diffs.dispose();
  });

  it("asks again for nothing once every open body is held", async () => {
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a", "b.js": "key-b" }, calls));
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js", "b.js"]) });
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js", "b.js"]) });
    expect(calls).toHaveLength(1);
    diffs.dispose();
  });

  it("shares the same request across repository instances on one request scope", async () => {
    const answer = deferred();
    const call = vi.fn(() => answer.promise);
    const requestScope = {};
    const first = fileDiffs.createFileDiffs({
      deviceId: "dev-1", entityId: "run-1", scope: { run_id: "run-1" }, call, requestScope,
    });
    const second = fileDiffs.createFileDiffs({
      deviceId: "dev-1", entityId: "run-1", scope: { run_id: "run-1" }, call, requestScope,
    });
    const firstSync = first.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    const secondSync = second.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
    answer.resolve({ files: [{ path: "a.js", ...bodyFor("a.js", "key-a") }] });
    await Promise.all([firstSync, secondSync]);
    expect(first.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    expect(second.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    first.dispose();
    second.dispose();
  });

  it("keeps matching entity requests separate when their request scopes differ", async () => {
    const call = vi.fn(async (_method, params) => ({
      files: params.paths.map((path) => ({ path, ...bodyFor(path, "key-a") })),
    }));
    const options = { deviceId: "dev-1", entityId: "run-1", scope: { run_id: "run-1" }, call };
    const first = fileDiffs.createFileDiffs({ ...options, requestScope: {} });
    const second = fileDiffs.createFileDiffs({ ...options, requestScope: {} });
    await Promise.all([
      first.sync({ status: TWO, openPaths: new Set(["a.js"]) }),
      second.sync({ status: TWO, openPaths: new Set(["a.js"]) }),
    ]);
    expect(call).toHaveBeenCalledTimes(2);
    first.dispose();
    second.dispose();
  });

  it("answers how many bodies it filled, so a caller repaints only on news", async () => {
    const diffs = mountDiffs(wireOver({ "a.js": "key-a" }, []));
    expect(await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) })).toBe(1);
    expect(await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) })).toBe(0);
    diffs.dispose();
  });

  it("caps each git.diff at the verb's path limit", async () => {
    const many = Array.from({ length: 60 }, (_unused, index) => `f${index}.js`);
    const keys = Object.fromEntries(many.map((path) => [path, `key-${path}`]));
    const shape = status(many.map((path) => statusFile(path, keys[path])));
    const calls = [];
    const diffs = mountDiffs(wireOver(keys, calls));
    await diffs.sync({ status: shape, openPaths: new Set(many) });
    expect(calls.map((call) => call.params.paths.length)).toEqual([50, 10]);
    diffs.dispose();
  });

  it("warms every file's body in one bounded pass, open or not", async () => {
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a", "b.js": "key-b" }, calls));
    await diffs.warm(TWO, { budget: 1 });
    expect(calls.map((call) => call.params.paths)).toEqual([["a.js"]]);
    expect(diffs.bodyOf("a.js")).toEqual(bodyFor("a.js", "key-a"));
    diffs.dispose();
  });

  it("stamps a background repository's git.diff with the request priority, and a foreground one with nothing", async () => {
    const envelopes = [];
    const call = vi.fn(async (method, params, envelope) => {
      envelopes.push(envelope);
      return { files: params.paths.map((path) => ({ path, ...bodyFor(path, "key-a") })) };
    });
    const background = fileDiffs.createFileDiffs({
      deviceId: "dev-1",
      entityId: "run-1",
      scope: { run_id: "run-1" },
      call,
      requestPriority: "background",
    });
    await background.warm(TWO, { budget: 1 });
    background.dispose();
    const foreground = mountDiffs(call);
    await foreground.sync({ status: TWO, openPaths: new Set(["b.js"]) });
    foreground.dispose();
    expect(envelopes[0]).toEqual({ priority: "background" });
    expect(envelopes.length).toBeGreaterThan(1);
    envelopes.slice(1).forEach((envelope) => expect(envelope).toEqual({}));
  });

  it("fetches nothing more once disposed", async () => {
    const calls = [];
    const diffs = mountDiffs(wireOver({ "a.js": "key-a" }, calls));
    diffs.dispose();
    await diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) });
    expect(calls).toEqual([]);
  });

  it("lets a rejected git.diff surface, holding no body for the paths it asked about", async () => {
    const failing = vi.fn(async () => {
      throw new Error("path is not readable through git.diff: .mcp.json");
    });
    const diffs = mountDiffs(failing);
    await expect(diffs.sync({ status: TWO, openPaths: new Set(["a.js"]) })).rejects.toThrow("not readable");
    expect(diffs.bodyOf("a.js")).toBeUndefined();
    diffs.dispose();
  });
});
