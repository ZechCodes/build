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
