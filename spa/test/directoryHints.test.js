// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const capabilities = vi.hoisted(() => ({ hints: true }));
vi.mock("../src/core/changeEvents.js", async (original) => ({ ...(await original()),
  bridgeCapabilities: () => ({ changes: { filesHints: capabilities.hints }, fs: { treePaged: true } }),
}));
let cache, pages, registry, mountFileTree, tree, host;
const deviceId = "synthetic-directory-hints", entityId = "synthetic-run";
const address = (sub, kind = "tree") => ({ deviceId, entityId, kind, sub });
const entry = (name, kind = "file") => ({ name, kind });
const listing = (path, entries) => ({ path, entries, has_more: false, next_cursor: null });
const disposers = [];
beforeEach(async () => {
  vi.resetModules(); capabilities.hints = true;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); pages = await import("../src/core/directoryPages.js");
  registry = await import("../src/core/syncVisibleScopes.js");
  ({ mountFileTree } = await import("../src/core/fileTree.js"));
  host = document.createElement("div"); document.body.appendChild(host);
  for (const path of ["", "src", "docs"]) await cache.writeCached(address(path), listing(path, path ? [entry("a.txt")] : [entry("src", "dir"), entry("docs", "dir")]));
});
afterEach(() => { tree?.dispose(); tree = null; disposers.splice(0).forEach((dispose) => dispose()); host.remove(); vi.restoreAllMocks(); });

it("chooses the independently negotiated hint kind without changing older bridge subscriptions", () => {
  expect(pages.chooseFilePushKind(deviceId)).toBe("files.hints");
  capabilities.hints = false;
  expect(pages.chooseFilePushKind(deviceId)).toBe("files");
});

it("marks touched cached directories stale and refreshes only a matching visible file reader", async () => {
  const readTree = vi.fn(async () => {}), readFile = vi.fn(async () => {}), hidden = vi.fn(async () => {});
  disposers.push(registry.registerSyncRead(() => address(""), readTree));
  disposers.push(registry.registerSyncRead(() => address("src/a.txt", "file"), readFile));
  disposers.push(registry.registerSyncRead(() => address("docs/a.txt", "file"), hidden));
  const files = { paths: ["src/a.txt"], truncated: false };
  await pages.applyDirectoryHint({ deviceId, entityId, files, active: () => true });
  expect((await cache.readCached(address("src"))).value.stale).toBe(true);
  expect((await cache.readCached(address(""))).value.stale).toBeUndefined();
  expect((await cache.readCached(address("docs"))).value.stale).toBeUndefined();
  expect(readTree).toHaveBeenCalledWith(expect.any(Function), { files });
  expect(readFile).toHaveBeenCalledTimes(1);
  expect(hidden).not.toHaveBeenCalled();
});

it("refreshes only first pages of visible touched directories and leaves a folded directory cold", async () => {
  const rpc = vi.fn(async (path) => listing(path, path ? [entry("current.txt")] : [entry("src", "dir"), entry("docs", "dir")]));
  tree = mountFileTree(host, { listingAddress: address, readsForItself: () => true, listDirectory: rpc, finePointer: () => true, onOpen: () => {} });
  await vi.waitFor(() => expect(host.querySelector('[data-path="src"]')).not.toBeNull());
  host.querySelector('[data-path="src"]').click();
  await vi.waitFor(() => expect(host.querySelector('[data-path="src/current.txt"]')).not.toBeNull());
  rpc.mockClear();
  await pages.applyDirectoryHint({ deviceId, entityId, files: { paths: ["src/current.txt"], truncated: false }, active: () => true });
  expect(rpc.mock.calls).toEqual([["src"]]);
  rpc.mockClear();
  await pages.applyDirectoryHint({ deviceId, entityId, files: { paths: [], truncated: true }, active: () => true });
  expect(rpc.mock.calls.map(([path]) => path).sort()).toEqual(["", "src"]);
  expect((await cache.readCached(address("src"))).value.stale).toBeUndefined();
  expect((await cache.readCached(address("docs"))).value.stale).toBe(true);
  host.hidden = true; rpc.mockClear();
  await pages.applyDirectoryHint({ deviceId, entityId, files: { paths: [], truncated: true }, active: () => true });
  expect(rpc).not.toHaveBeenCalled();
});

it("refreshes every visible expanded first page for a truncated hint while folded descendants stay cold", async () => {
  let newEntry = false;
  const entries = (path) => ({
    "": [entry("src", "dir"), entry("docs", "dir"), entry("archive", "dir")],
    src: [entry("current.txt")], docs: [entry("guide.txt"), ...(newEntry ? [entry("new.txt")] : [])],
    archive: [entry("old", "dir")], "archive/old": [entry("history.txt")],
  })[path];
  const rpc = vi.fn(async (path) => listing(path, entries(path)));
  tree = mountFileTree(host, { listingAddress: address, readsForItself: () => true,
    listDirectory: rpc, finePointer: () => true, onOpen: () => {} });
  await vi.waitFor(() => expect(host.querySelector('[data-path="src"]')).not.toBeNull());
  for (const [dir, child] of [["src", "src/current.txt"], ["docs", "docs/guide.txt"],
    ["archive", "archive/old"], ["archive/old", "archive/old/history.txt"]]) {
    host.querySelector(`[data-path="${dir}"]`).click();
    await vi.waitFor(() => expect(host.querySelector(`[data-path="${child}"]`)).not.toBeNull());
  }
  host.querySelector('[data-path="archive"]').click();
  expect(host.querySelector('[data-path="archive/old/history.txt"]')).toBeNull();
  tree.setOpenPath("src/current.txt");
  const folded = (await cache.readCached(address("archive/old"))).value;
  expect(host.querySelector('[data-path="docs/new.txt"]')).toBeNull();
  newEntry = true;
  rpc.mockClear();
  await pages.applyDirectoryHint({ deviceId, entityId,
    files: { paths: ["docs/new.txt"], truncated: true }, active: () => true });
  expect(rpc.mock.calls.map(([path]) => path).sort()).toEqual(["", "docs", "src"]);
  expect(rpc.mock.calls.every((args) => args.length === 1)).toBe(true);
  expect((await cache.readCached(address("archive/old"))).value).toEqual({ ...folded, stale: true });
  expect((await cache.readCached(address("archive"))).value.stale).toBe(true);
  expect(host.querySelector('[data-path="docs/new.txt"]')).not.toBeNull();
  expect(host.querySelector('[data-path="src/current.txt"]')?.getAttribute("aria-current")).toBe("true");
  host.hidden = true; rpc.mockClear();
  await pages.applyDirectoryHint({ deviceId, entityId,
    files: { paths: ["docs/new.txt"], truncated: true }, active: () => true });
  expect(rpc).not.toHaveBeenCalled();
  expect((await cache.readCached(address("src"))).value.stale).toBe(true);
  expect((await cache.readCached(address("docs"))).value.stale).toBe(true);
});

it.each([{ paths: ["../foreign"] }, { paths: [] }])("treats foreign or unknown paths as an invalidation without using them as cache addresses: $paths", async ({ paths }) => {
  const hidden = vi.fn();
  disposers.push(registry.registerSyncRead(() => address(""), hidden, () => false));
  await pages.applyDirectoryHint({ deviceId, entityId, files: { paths, truncated: false }, active: () => true });
  expect((await cache.readCached(address("docs"))).value.stale).toBe(true);
  expect(hidden).not.toHaveBeenCalled();
  expect(await cache.readCached(address("../foreign"))).toBeUndefined();
});
