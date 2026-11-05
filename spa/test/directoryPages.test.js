// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { setImmediate as immediate } from "node:timers";
const support = vi.hoisted(() => ({ paged: true, lazy: true }));
vi.mock("../src/core/changeEvents.js", async (original) => ({ ...(await original()),
  bridgeCapabilities: () => ({ fs: { treePaged: support.paged } }),
}));
vi.mock("../src/core/scopedSyncRuntime.js", async (original) => ({ ...(await original()),
  lazyViewReadsEnabled: () => support.lazy,
}));
let cache, files, host, scopeFor, renderFilesTab;
const address = { deviceId: "directory-pages", entityId: "synthetic-run", kind: "tree", sub: "" };
const tree = (name, next_cursor = null) => ({ path: "", entries: [{ name, kind: "file", size: 1 }], has_more: Boolean(next_cursor), next_cursor });
beforeEach(async () => {
  vi.resetModules(); support.paged = true; support.lazy = true;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ scopeFor } = await import("../src/core/cacheScope.js"));
  ({ renderFilesTab } = await import("../src/views/files.js"));
  host = document.createElement("div"); document.body.appendChild(host);
});
afterEach(() => { files?.dispose(); files = null; host.remove(); vi.restoreAllMocks(); });
const mount = (callRpc, options = {}) => files = renderFilesTab(host, { scope: { run_id: "synthetic-run" }, cacheScope: scopeFor(address.deviceId), callRpc, ...options });

it("paints a cached selected body without a loading flash and registers its visible refresh", async () => {
  const body = (text) => ({ path: "cached.txt", size: text.length, truncated: false, mime: "text/plain", content_b64: Buffer.from(text).toString("base64"), editable: true, encoding: "utf-8", revision: text });
  await cache.writeCached(address, tree("cached.txt"));
  await cache.writeCached({ ...address, kind: "file", sub: "cached.txt" }, { file: body("saved content"), openedAt: Date.now() });
  let release, loading = false;
  const pending = new Promise((resolve) => { release = resolve; });
  const observer = new MutationObserver(() => { loading ||= Boolean(host.querySelector('.throbber[aria-label="loading"]')); });
  observer.observe(host, { childList: true, subtree: true });
  const call = vi.fn(async (method) => method === "fs.read" ? pending : tree("cached.txt"));
  mount(call, { openAt: { path: "cached.txt" } });
  await vi.waitFor(() => expect(host.textContent).toContain("saved content"));
  expect(loading).toBe(false);
  await vi.waitFor(() => expect(call.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(1));
  const { visibleSyncReads } = await import("../src/core/syncVisibleScopes.js");
  const read = visibleSyncReads(address.deviceId).find(({ key }) => key.kind === "file");
  expect(read?.key.sub).toBe("cached.txt");
  release(body("new content"));
  await vi.waitFor(() => expect(host.textContent).toContain("new content"));
  await read.read();
  expect(call.mock.calls.filter(([method]) => method === "fs.read")).toHaveLength(2);
  observer.disconnect();
});

it("negotiates bounded first and continuation reads from a cached covered checkout", async () => {
  await cache.writeCached(address, tree("cached.txt"));
  const call = vi.fn(async (_method, params) => params.cursor ? tree("later.txt") : tree("first.txt", "opaque"));
  mount(call);
  await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
  expect(call).toHaveBeenCalledWith("fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100 });
  host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(host.querySelector('[data-path="later.txt"]')).not.toBeNull());
  expect(call).toHaveBeenLastCalledWith("fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100, cursor: "opaque" });
});

it("keeps the exact legacy request shape without the capability", async () => {
  support.paged = false; support.lazy = false;
  const call = vi.fn(async () => ({ path: "", entries: [] }));
  mount(call);
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("fs.tree", { run_id: "synthetic-run", path: "" }));
});

it("keeps a scoped checkout's cached listing when its open-time revalidation fails", async () => {
  await cache.writeCached(address, tree("cached.txt"));
  const call = vi.fn(async () => { throw new Error("temporary read failure"); });
  mount(call);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  await new Promise(immediate);
  expect(host.querySelector('[data-path="cached.txt"]')).not.toBeNull();
  expect(host.textContent).not.toContain("cannot list");
  expect((await cache.readCached(address)).value.entries[0].name).toBe("cached.txt");
});

it("drops cached paged continuations on a 3.35 → legacy → 3.35 capability rollback", async () => {
  support.paged = false;
  await cache.writeCached(address, tree("cached.txt", "foreign-old-cursor"));
  const call = vi.fn(async () => ({ path: "", entries: [{ name: "legacy.txt", kind: "file" }] }));
  mount(call);
  await vi.waitFor(() => expect(host.querySelector('[data-path="legacy.txt"]')).not.toBeNull());
  expect(call.mock.calls[0]).toEqual(["fs.tree", { run_id: "synthetic-run", path: "" }]);
  files.dispose();
  support.paged = true;
  call.mockImplementation(async () => tree("upgraded.txt", "fresh-cursor"));
  mount(call);
  await vi.waitFor(() => expect(host.querySelector('[data-path="upgraded.txt"]')).not.toBeNull());
  expect(call).toHaveBeenLastCalledWith("fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100 });
  expect((await cache.readCached(address)).value.next_cursor).toBe("fresh-cursor");
});

// The absent-birthtime provider has the same wire DTO as every provider. These
// scripted contract responses compose with Rust's real held-handle/TTL tests;
// the SPA never sees provider metadata and does not simulate its lease registry.
it("opens an absent-birthtime provider cold and reads its bounded pages to completion only on demand", async () => {
  const names = Array.from({ length: 205 }, (_, index) => `file-${String(index).padStart(3, "0")}.txt`);
  const response = (from, to, next_cursor = null) => ({ path: "",
    entries: names.slice(from, to).map((name) => ({ name, kind: "file", size: 1 })),
    has_more: Boolean(next_cursor), next_cursor });
  const call = vi.fn().mockResolvedValueOnce(response(0, 100, "opaque-held-directory-page-1"))
    .mockResolvedValueOnce(response(100, 200, "opaque-held-directory-page-2"))
    .mockResolvedValueOnce(response(200, 205));
  mount(call);
  await vi.waitFor(() => expect(host.querySelectorAll(".ffile")).toHaveLength(100));
  expect(call.mock.calls).toEqual([["fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100 }]]);
  host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(host.querySelectorAll(".ffile")).toHaveLength(200));
  expect(call).toHaveBeenCalledTimes(2);
  host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(host.querySelectorAll(".ffile")).toHaveLength(205));
  expect(call.mock.calls.map(([, params]) => params)).toEqual([
    { run_id: "synthetic-run", path: "", paged: true, limit: 100 },
    { run_id: "synthetic-run", path: "", paged: true, limit: 100, cursor: "opaque-held-directory-page-1" },
    { run_id: "synthetic-run", path: "", paged: true, limit: 100, cursor: "opaque-held-directory-page-2" },
  ]);
  expect(host.querySelector("[data-tree-more]")).toBeNull();
  const record = (await cache.readCached(address)).value;
  expect(record.entries.map(({ name }) => name)).toEqual(names);
  expect(record).toMatchObject({ has_more: false, next_cursor: null });
});

it.each(["replaced", "evicted", "expired"])("quietly refreshes an absent-birthtime provider's %s cursor while retaining cached Files rows", async (refusal) => {
  const held = tree("cached.txt", `opaque-${refusal}-cursor`);
  await cache.writeCached(address, held);
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const message = `synthetic ${refusal} directory cursor`;
  const call = vi.fn().mockResolvedValueOnce(held)
    .mockRejectedValueOnce(Object.assign(new Error(message), { code: "invalid_params" }))
    .mockImplementationOnce(() => pending);
  const paintedFailures = [];
  const observer = new MutationObserver(() => {
    if (host.textContent.includes(message) || host.querySelector('.throbber[aria-label="loading"]')) paintedFailures.push(host.textContent);
  });
  observer.observe(host, { childList: true, subtree: true });
  try {
    mount(call);
    await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
    host.querySelector("[data-tree-more]").click();
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(3));
    expect(host.querySelector('[data-path="cached.txt"]')).not.toBeNull();
    expect((await cache.readCached(address)).value).toEqual(held);
    expect(call.mock.calls[1]).toEqual(["fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100, cursor: held.next_cursor }]);
    expect(call.mock.calls[2]).toEqual(["fs.tree", { run_id: "synthetic-run", path: "", paged: true, limit: 100 }]);
    release(tree("fresh.txt", "opaque-fresh-cursor"));
    await vi.waitFor(() => expect(host.querySelector('[data-path="fresh.txt"]')).not.toBeNull());
    expect((await cache.readCached(address)).value.next_cursor).toBe("opaque-fresh-cursor");
    expect(paintedFailures).toEqual([]);
    expect(call).toHaveBeenCalledTimes(3);
  } finally { release(tree("fresh.txt", "opaque-fresh-cursor")); observer.disconnect(); }
});
