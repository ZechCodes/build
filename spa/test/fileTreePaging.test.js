// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, mountFileTree, tree, host;
const address = (dir) => ({ deviceId: "directory-pages", entityId: "synthetic-root", kind: "tree", sub: dir });
const page = (names, next_cursor = null) => ({ path: "", entries: names.map((name) => ({ name, kind: "file", size: 1 })), has_more: Boolean(next_cursor), next_cursor });
const paths = () => [...host.querySelectorAll(".ffile")].map((row) => row.dataset.path);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ mountFileTree } = await import("../src/core/fileTree.js"));
  host = document.createElement("div"); document.body.appendChild(host);
});
afterEach(() => { tree?.dispose(); tree = null; host.remove(); vi.restoreAllMocks(); });

const mount = (listDirectory, readsForItself = () => true) => tree = mountFileTree(host, {
  listingAddress: address, readsForItself, listDirectory, finePointer: () => true, onOpen: vi.fn(), keepHeldOnError: true,
});

it("paints cached entries while the first paged read is pending, then loads older pages only on demand", async () => {
  await cache.writeCached(address(""), page(["cached.txt"]));
  let release;
  const first = new Promise((resolve) => { release = resolve; });
  const listing = vi.fn((_dir, continuation) => continuation?.cursor ? Promise.resolve(page(["b.txt"])) : first);
  mount(listing);
  await vi.waitFor(() => expect(paths()).toEqual(["cached.txt"]));
  expect(listing).toHaveBeenCalledTimes(1);
  release(page(["a.txt"], "opaque-directory-cursor"));
  await vi.waitFor(() => expect(paths()).toEqual(["a.txt"]));
  expect(listing).toHaveBeenCalledTimes(1);
  const more = host.querySelector("[data-tree-more]");
  expect(more?.textContent).toContain("Load more");
  tree.setOpenPath("b.txt");
  more.click();
  await vi.waitFor(() => expect(paths()).toEqual(["a.txt", "b.txt"]));
  expect(listing).toHaveBeenLastCalledWith("", { cursor: "opaque-directory-cursor" });
  expect(host.querySelector("[data-tree-more]")).toBeNull();
  expect(host.querySelector('[data-path="b.txt"]')?.getAttribute("aria-current")).toBe("true");
  expect((await cache.readCached(address(""))).value).toMatchObject({ entries: page(["a.txt", "b.txt"]).entries, has_more: false, next_cursor: null });
});

it("keeps cached content and continuation after a failed older page and fences disposal", async () => {
  const listing = vi.fn().mockResolvedValueOnce(page(["a.txt"], "older")).mockRejectedValueOnce(new Error("directory changed"));
  mount(listing);
  await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
  host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(listing).toHaveBeenCalledTimes(2));
  expect(paths()).toEqual(["a.txt"]);
  expect((await cache.readCached(address(""))).value.next_cursor).toBe("older");
  let release;
  listing.mockImplementationOnce(() => new Promise((resolve) => { release = resolve; }));
  host.querySelector("[data-tree-more]").click();
  await vi.waitFor(() => expect(listing).toHaveBeenCalledTimes(3));
  tree.dispose(); release(page(["late.txt"]));
  await Promise.resolve();
  expect((await cache.readCached(address(""))).value.entries.map(({ name }) => name)).toEqual(["a.txt"]);
});

it("retains legacy cached-list behavior when the checkout is still maintained by legacy sync", async () => {
  await cache.writeCached(address(""), { path: "", entries: page(["legacy.txt"]).entries });
  const listing = vi.fn();
  mount(listing, () => false);
  await vi.waitFor(() => expect(paths()).toEqual(["legacy.txt"]));
  expect(listing).not.toHaveBeenCalled();
  expect(host.querySelector("[data-tree-more]")).toBeNull();
});

it("does not let an older page overwrite a replacement cursor written in the same clock tick", async () => {
  vi.spyOn(Date, "now").mockReturnValue(42);
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const listing = vi.fn().mockResolvedValueOnce(page(["a.txt"], "old-cursor")).mockImplementationOnce(() => pending);
  mount(listing);
  await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
  const read = tree.more("");
  await vi.waitFor(() => expect(listing).toHaveBeenCalledTimes(2));
  await cache.writeCached(address(""), page(["a.txt", "c.txt"], "replacement-cursor"));
  release(page(["b.txt"]));
  await read;
  expect((await cache.readCached(address(""))).value).toEqual(page(["a.txt", "c.txt"], "replacement-cursor"));
});

it("recovers a refused directory cursor through a fresh first page while keeping cached rows", async () => {
  const refused = Object.assign(new Error("directory replaced"), { code: "invalid_params" });
  let release;
  const first = new Promise((resolve) => { release = resolve; });
  const listing = vi.fn().mockResolvedValueOnce(page(["cached.txt"], "retired-cursor"))
    .mockRejectedValueOnce(refused).mockImplementationOnce(() => first);
  mount(listing);
  await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
  const older = tree.more("");
  await vi.waitFor(() => expect(listing).toHaveBeenCalledTimes(3));
  expect(paths()).toEqual(["cached.txt"]);
  expect(listing).toHaveBeenLastCalledWith("");
  release(page(["replacement.txt"], "fresh-cursor"));
  await older;
  expect((await cache.readCached(address(""))).value.next_cursor).toBe("fresh-cursor");
});

it("keeps the reading place when repaint restores keyboard focus after loading more", async () => {
  const listing = vi.fn().mockResolvedValueOnce(page(["a.txt"], "older"))
    .mockResolvedValueOnce(page(["b.txt"]));
  mount(listing);
  await vi.waitFor(() => expect(host.querySelector("[data-tree-more]")).not.toBeNull());
  tree.setOpenPath("a.txt");
  const nativeFocus = HTMLElement.prototype.focus;
  // jsdom has no layout scrolling. Model the native focus scroll that would
  // bring the selected first row back into view above this reading position.
  vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(function (options) {
    nativeFocus.call(this, options);
    if (!options?.preventScroll) host.scrollTop = 0;
  });
  host.scrollTop = 75;
  host.querySelector("[data-tree-more]").focus({ preventScroll: true });
  await tree.more("");
  await vi.waitFor(() => expect(host.querySelector('[data-path="b.txt"]')).not.toBeNull());
  expect(host.scrollTop).toBe(75);
  expect(host.querySelector('[data-path="a.txt"]')?.getAttribute("aria-current")).toBe("true");
});

const directoryWindow = (from, next_cursor = null) => ({ path: "",
  entries: Array.from({ length: 100 }, (_, index) => ({ name: `file-${String(from + index).padStart(3, "0")}.txt`, kind: "file", size: 1 })),
  has_more: Boolean(next_cursor), next_cursor });
const browsingPages = (_dir, continuation) => !continuation?.cursor
  ? directoryWindow(0, "opaque-window-100")
  : continuation.cursor === "opaque-window-100" ? directoryWindow(100, "opaque-window-200") : directoryWindow(200);

it("retains a three-page selected row when a child hint refreshes the loaded directory window", async () => {
  const listing = vi.fn(async (...args) => browsingPages(...args));
  mount(listing);
  await vi.waitFor(() => expect(paths()).toHaveLength(100));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(200));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(300));
  tree.setOpenPath("file-250.txt");
  listing.mockClear();
  const missingSelection = [];
  const observer = new MutationObserver(() => {
    if (!host.querySelector('[data-path="file-250.txt"][aria-current="true"]')) missingSelection.push(true);
  });
  observer.observe(host, { childList: true, subtree: true });
  try {
    await tree.invalidate({ paths: ["file-050.txt"], truncated: false });
    await vi.waitFor(() => expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true"));
    expect(listing.mock.calls).toEqual([[""], ["", { cursor: "opaque-window-100" }], ["", { cursor: "opaque-window-200" }]]);
    expect((await cache.readCached(address(""))).value.entries).toHaveLength(300);
    expect(missingSelection).toEqual([]);
  } finally { observer.disconnect(); }
});

it("reveals a requested file beyond the first directory page without draining after it is found", async () => {
  const listing = vi.fn(async (...args) => browsingPages(...args));
  mount(listing);
  await vi.waitFor(() => expect(paths()).toHaveLength(100));
  await tree.reveal("file-250.txt");
  await vi.waitFor(() => expect(host.querySelector('[data-path="file-250.txt"]')).not.toBeNull());
  expect(listing.mock.calls).toEqual([[""], ["", { cursor: "opaque-window-100" }], ["", { cursor: "opaque-window-200" }]]);
});

it("preserves the selected window when an older paged cache has no saved page count", async () => {
  const cached = { path: "", entries: [0, 100, 200].flatMap((from) => directoryWindow(from).entries),
    has_more: false, next_cursor: null };
  await cache.writeCached(address(""), cached);
  let release;
  const first = new Promise((resolve) => { release = resolve; });
  const listing = vi.fn(async (dir, continuation) => {
    if (!continuation?.cursor) await first;
    return browsingPages(dir, continuation);
  });
  try {
    mount(listing);
    tree.setOpenPath("file-250.txt");
    await vi.waitFor(() => expect(paths()).toHaveLength(300));
    expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true");
    release();
    await vi.waitFor(() => expect(listing).toHaveBeenCalledTimes(3));
    expect((await cache.readCached(address(""))).value.entries).toHaveLength(300);
    expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true");
  } finally { release(); }
});

it("retains the loaded sort window when the old tail file becomes a same-name directory", async () => {
  let changed = false;
  const listing = vi.fn(async (dir, continuation) => {
    if (!changed) return browsingPages(dir, continuation);
    if (continuation?.cursor === "opaque-sort-99") return directoryWindow(99, "opaque-sort-199");
    if (continuation?.cursor === "opaque-sort-199") return directoryWindow(199);
    return { ...directoryWindow(0, "opaque-sort-99"), entries: [
      { name: "file-299.txt", kind: "dir" }, ...directoryWindow(0).entries.slice(0, 99),
    ] };
  });
  mount(listing);
  await vi.waitFor(() => expect(paths()).toHaveLength(100));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(200));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(300));
  tree.setOpenPath("file-250.txt");
  changed = true; listing.mockClear();
  await tree.invalidate({ paths: ["file-299.txt"], truncated: false });
  expect(listing.mock.calls).toEqual([[""], ["", { cursor: "opaque-sort-99" }], ["", { cursor: "opaque-sort-199" }]]);
  await vi.waitFor(() => expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true"));
  const entries = (await cache.readCached(address(""))).value.entries;
  expect(entries.find(({ name }) => name === "file-299.txt").kind).toBe("dir");
  expect(entries.some(({ name }) => name === "file-250.txt")).toBe(true);
});

async function mountThreePageWindow(listing) {
  mount(listing);
  await vi.waitFor(() => expect(paths()).toHaveLength(100));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(200));
  await tree.more("");
  await vi.waitFor(() => expect(paths()).toHaveLength(300));
  tree.setOpenPath("file-250.txt");
}

function internalRefusalProvider({ repeated = false, refuseAt = "first" } = {}) {
  let refreshing = false, firstReads = 0, release;
  const freshHead = new Promise((resolve) => { release = resolve; });
  const refusal = Object.assign(new Error("synthetic internal cursor lease expired"), { code: "invalid_params" });
  const refusedHead = () => directoryWindow(0, refuseAt === "second" ? "opaque-before-refusal" : `opaque-expired-${firstReads}`);
  const listing = vi.fn(async (dir, continuation) => {
    if (!refreshing) return browsingPages(dir, continuation);
    if (!continuation?.cursor) {
      firstReads += 1;
      if (firstReads > 2) throw new Error("held-window restart budget exceeded");
      return firstReads === 1 || repeated ? refusedHead() : freshHead;
    }
    if (continuation.cursor === "opaque-before-refusal") return directoryWindow(100, `opaque-expired-${firstReads}`);
    if (continuation.cursor.startsWith("opaque-expired-")) throw refusal;
    if (continuation.cursor === "opaque-fresh-100") return directoryWindow(100, "opaque-fresh-200");
    if (continuation.cursor === "opaque-fresh-200") return directoryWindow(200);
    throw new Error("Unexpected synthetic directory continuation");
  });
  return { listing, firstReads: () => firstReads,
    begin() { refreshing = true; listing.mockClear(); },
    release() {
      const page = directoryWindow(0, "opaque-fresh-100");
      page.entries[0].size = 2;
      release(page);
    },
  };
}

it.each(["first", "second"])("restarts a held window from a fresh first page after an internal continuation refusal (%s continuation)", async (refuseAt) => {
  const provider = internalRefusalProvider({ refuseAt });
  await mountThreePageWindow(provider.listing);
  const before = await cache.readCached(address(""));
  const displaced = [];
  const observer = new MutationObserver(() => {
    if (!host.querySelector('[data-path="file-250.txt"][aria-current="true"]')
      || host.textContent.includes("synthetic internal cursor lease expired")) displaced.push(true);
  });
  observer.observe(host, { childList: true, subtree: true });
  provider.begin();
  const refresh = tree.invalidate({ paths: ["file-050.txt"], truncated: false });
  try {
    await vi.waitFor(() => expect(provider.firstReads()).toBe(2));
    expect(await cache.readCached(address(""))).toEqual(before);
    expect(paths()).toHaveLength(300);
    provider.release();
    await refresh;
    await vi.waitFor(async () => expect((await cache.readCached(address(""))).value.entries[0].size).toBe(2));
    const firstAttempt = refuseAt === "second"
      ? [[""], ["", { cursor: "opaque-before-refusal" }], ["", { cursor: "opaque-expired-1" }]]
      : [[""], ["", { cursor: "opaque-expired-1" }]];
    expect(provider.listing.mock.calls).toEqual([...firstAttempt, [""],
      ["", { cursor: "opaque-fresh-100" }], ["", { cursor: "opaque-fresh-200" }]]);
    expect((await cache.readCached(address(""))).value.entries).toHaveLength(300);
    expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true");
    expect(displaced).toEqual([]);
  } finally { provider.release(); observer.disconnect(); await refresh; }
});

it("bounds repeated internal continuation refusals while retaining the painted cached window", async () => {
  const provider = internalRefusalProvider({ repeated: true });
  await mountThreePageWindow(provider.listing);
  const before = await cache.readCached(address(""));
  provider.begin();
  await tree.invalidate({ paths: ["file-050.txt"], truncated: false });
  expect(provider.firstReads()).toBe(2);
  expect(provider.listing.mock.calls).toEqual([[""], ["", { cursor: "opaque-expired-1" }],
    [""], ["", { cursor: "opaque-expired-2" }]]);
  expect(await cache.readCached(address(""))).toEqual(before);
  expect(paths()).toHaveLength(300);
  expect(host.querySelector('[data-path="file-250.txt"]')?.getAttribute("aria-current")).toBe("true");
  expect(host.textContent).not.toContain("synthetic internal cursor lease expired");
});

it.each(["hidden", "retired", "new cache write"])("preserves the %s fence across the held-window retry boundary", async (fence) => {
  const provider = internalRefusalProvider();
  await mountThreePageWindow(provider.listing);
  let expected = await cache.readCached(address("")), owner = true;
  provider.begin();
  const refresh = tree.invalidate({ paths: ["file-050.txt"], truncated: false }, () => owner && !host.hidden);
  try {
    await vi.waitFor(() => expect(provider.firstReads()).toBe(2));
    if (fence === "hidden") host.hidden = true;
    if (fence === "retired") owner = false;
    if (fence === "new cache write") {
      await cache.writeCached(address(""), { ...expected.value,
        entries: expected.value.entries.map((entry) => ({ ...entry, size: 7 })) });
      expected = await cache.readCached(address(""));
    }
    provider.release();
    await refresh;
    expect(await cache.readCached(address(""))).toEqual(expected);
    expect(provider.listing).toHaveBeenCalledTimes(fence === "new cache write" ? 5 : 3);
  } finally { provider.release(); await refresh; }
});
