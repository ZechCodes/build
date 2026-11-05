// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const greeting = vi.hoisted(() => ({ hints: false, listeners: new Set() }));
vi.mock("../src/core/changeEvents.js", async (original) => ({ ...(await original()),
  bridgeCapabilities: () => ({ changes: { filesHints: greeting.hints }, fs: { treePaged: greeting.hints } }),
  onBridgeGreeted: (listener) => { greeting.listeners.add(listener); return () => greeting.listeners.delete(listener); },
}));
const deviceId = "synthetic-legacy-directory", entityId = "synthetic-run";
const address = (sub, kind = "tree") => ({ deviceId, entityId, kind, sub });
const entry = (name, kind = "file") => ({ name, kind });
const root = (name) => ({ path: "", entries: [entry("src", "dir"), entry(name)] });
const page = (value) => ({ ...value, has_more: true, next_cursor: "retired-page" });
let cache, pages, registry, host, tree;
const stops = [];
beforeEach(async () => {
  vi.resetModules(); greeting.hints = false; greeting.listeners.clear();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  pages = await import("../src/core/directoryPages.js");
  registry = await import("../src/core/syncVisibleScopes.js");
  host = document.createElement("div"); document.body.appendChild(host);
  await cache.writeCached(address(""), page(root("cached.txt")));
  await cache.writeCached(address("src"), { path: "src", entries: [entry("old.txt")] });
  await cache.writeCached(address("folded"), { path: "folded", entries: [entry("held.txt")] });
});
afterEach(() => { tree?.dispose(); tree = null; stops.splice(0).forEach((stop) => stop()); host.remove(); vi.restoreAllMocks(); });
const mount = async (rpc, readsForItself = () => false) => {
  const { mountFileTree } = await import("../src/core/fileTree.js");
  tree = mountFileTree(host, { listingAddress: address, readsForItself,
    listDirectory: rpc, finePointer: () => true, onOpen: () => {} });
  await vi.waitFor(() => expect(host.querySelector('[data-path="cached.txt"]')).not.toBeNull());
};

it("uses a scoped 3.34 pushed root before a deeper read completes without a duplicate root RPC", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const rpc = vi.fn().mockResolvedValueOnce({ ...page(root("cached.txt")), next_cursor: "open-cursor" })
    .mockResolvedValueOnce({ path: "src", entries: [entry("open.txt")] }).mockImplementation(() => held);
  const file = vi.fn(async () => {});
  await mount(rpc, () => true);
  await vi.waitFor(async () => expect((await cache.readCached(address(""))).value.next_cursor).toBe("open-cursor"));
  host.querySelector('[data-path="src"]').click();
  await vi.waitFor(() => expect(host.querySelector('[data-path="src/open.txt"]')).not.toBeNull());
  rpc.mockClear();
  stops.push(registry.registerSyncRead(() => address("src/current.txt", "file"), file));
  const pending = pages.applyDirectoryHint({ deviceId, entityId,
    files: { root: root("pushed.txt"), paths: ["pushed.txt", "src/current.txt"] } });
  await vi.waitFor(() => expect(host.querySelector('[data-path="pushed.txt"]')).not.toBeNull());
  expect(rpc.mock.calls).toEqual([["src"]]);
  expect(file).toHaveBeenCalledTimes(1);
  expect((await cache.readCached(address(""))).value).toEqual(root("pushed.txt"));
  expect(host.querySelector('[data-tree-more=""]')).toBeNull();
  release({ path: "src", entries: [entry("current.txt")] }); await pending;
  await vi.waitFor(() => expect(host.querySelector('[data-path="src/current.txt"]')).not.toBeNull());
});

it.each(["hidden", "folded"])("keeps a %s root stale without admitting its pushed body or reading", async (visibility) => {
  const rpc = vi.fn(); await mount(rpc);
  if (visibility === "hidden") host.hidden = true;
  else host.style.display = "none";
  await pages.applyDirectoryHint({ deviceId, entityId, files: { root: root("invisible.txt"), paths: [], truncated: true } });
  expect((await cache.readCached(address(""))).value).toEqual({ ...page(root("cached.txt")), stale: true });
  expect((await cache.readCached(address("folded"))).value.stale).toBe(true);
  expect(rpc).not.toHaveBeenCalled();
});

it("ignores an injected root from a currently hint-capable 3.35 bridge", async () => {
  greeting.hints = true;
  const fresh = root("read.txt"), rpc = vi.fn(async () => fresh); await mount(rpc);
  await pages.applyDirectoryHint({ deviceId, entityId, files: { root: root("injected.txt"), paths: [], truncated: true } });
  expect(rpc.mock.calls).toEqual([[""]]);
  expect((await cache.readCached(address(""))).value.entries).toEqual(fresh.entries);
  expect(host.querySelector('[data-path="injected.txt"]')).toBeNull();
});

it.each([{ path: "foreign", entries: [] }, { path: "", entries: null }])("does not accept a foreign or malformed legacy root: $path", async (body) => {
  const fresh = root("read.txt"), rpc = vi.fn(async () => fresh); await mount(rpc);
  await pages.applyDirectoryHint({ deviceId, entityId, files: { root: body, paths: [], truncated: true } });
  expect(rpc.mock.calls).toEqual([[""]]);
  expect((await cache.readCached(address(""))).value.entries).toEqual(fresh.entries);
});

it.each(["inactive", "regreeted", "folded", "newer-write"])("fences legacy root admission when %s while its cache transaction is pending", async (edge) => {
  let active = true, visible = true;
  const reader = vi.fn(async () => {});
  stops.push(registry.registerSyncTree(() => address(""), () => [""], () => visible));
  stops.push(registry.registerSyncRead(() => address(""), reader, () => visible));
  const merge = cache.mergeCachedRecordsTogether;
  vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementationOnce(async (...args) => {
    if (edge === "inactive") active = false;
    if (edge === "regreeted") for (const listener of greeting.listeners) listener(deviceId);
    if (edge === "folded") visible = false;
    if (edge === "newer-write") await cache.writeCached(address(""), root("newer.txt"));
    return merge(...args);
  });
  await pages.applyDirectoryHint({ deviceId, entityId, active: () => active,
    files: { root: root("obsolete.txt"), paths: [], truncated: true } });
  const held = (await cache.readCached(address(""))).value;
  expect(held.entries).toEqual(root(edge === "newer-write" ? "newer.txt" : "cached.txt").entries);
  if (edge === "folded") expect(held.stale).toBe(true);
  else expect(held.stale).toBeUndefined();
  if (edge !== "newer-write") expect(reader).not.toHaveBeenCalled();
  expect(greeting.listeners.size).toBe(0);
});

it("fences an older open-time root read after a complete legacy root is cached", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const rpc = vi.fn(() => held);
  const { mountFileTree } = await import("../src/core/fileTree.js");
  tree = mountFileTree(host, { listingAddress: address, readsForItself: () => true,
    listDirectory: rpc, finePointer: () => true, onOpen: () => {} });
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  await pages.applyDirectoryHint({ deviceId, entityId, files: { root: root("pushed.txt"), paths: [], truncated: true } });
  await vi.waitFor(() => expect(host.querySelector('[data-path="pushed.txt"]')).not.toBeNull());
  expect(rpc).toHaveBeenCalledTimes(1);
  release(root("late.txt"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect((await cache.readCached(address(""))).value).toEqual(root("pushed.txt"));
});
