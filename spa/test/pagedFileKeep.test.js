// @vitest-environment jsdom
// #284: the paged viewer reads its next page, checks that the file's record
// still names the version it is of, and keeps the page. A push landing
// between that check and the write must keep its pages: a page of the old
// version at the same offset would be written over the push's.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let pages, cache, view;

const head = { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "big.log" };
const b64 = (text) => Buffer.from(text).toString("base64");
const page = (of, offset, text) => ({ of, offset, end: offset + text.length, total: 20, body: b64(text) });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  pages = await import("../src/core/bodyPages.js");
});

afterEach(() => {
  view?.dispose();
  view = null;
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

/** Land `push` on the next read of the file's own record: in that read's own
 *  success, so its write is asked for before the reader can ask for any
 *  write of its own. */
const pushOnNextRecordRead = (push) => {
  const get = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function read(key) {
    const request = get.call(this, key);
    if (String(key).endsWith("|file|big.log")) {
      spy.mockRestore();
      request.addEventListener("success", () => void push());
    }
    return request;
  });
};

/** A newer version stored the way a push stores it: its record and every
 *  page, letting go of the other versions', in one transaction. */
const pushNewer = () => cache.writeCachedIfStill({
  puts: [
    pages.bodyPagePut(head, page("v2", 0, "new-first\n")),
    pages.bodyPagePut(head, page("v2", 10, "new-secnd\n")),
    { address: head, value: { file: { paged: true, of: "v2", size: 20 } } },
  ],
  drop: pages.bodyPagesDrop(head),
});

it("keeps a push's pages that land between the page read's check and its write", async () => {
  const { mountPagedFile } = await import("../src/core/pagedFileView.js");
  await cache.writeCached(head, { file: { paged: true, of: "v1", size: 20 } });
  await pages.writeBodyPage(head, page("v1", 0, "old-first\n"));
  const scroller = document.createElement("div");
  document.body.append(scroller);
  const paint = vi.fn(() => true);
  view = mountPagedFile(scroller, {
    head,
    file: { paged: true, of: "v1", size: 20 },
    readPage: async (offset) => page("v1", offset, "old-secnd\n"),
    painter: { paint },
  });
  await vi.waitFor(() => expect(paint).toHaveBeenCalled());

  pushOnNextRecordRead(pushNewer);
  const moved = await view.more();

  const held = await pages.readBodyPages(head, "v2");
  expect(held.complete).toBe(true);
  expect(held.pages.map((kept) => Buffer.from(kept.body, "base64").toString())).toEqual(["new-first\n", "new-secnd\n"]);
  expect(moved).toBe(false);
});

it("still keeps the next page while the record names its version", async () => {
  const { mountPagedFile } = await import("../src/core/pagedFileView.js");
  await cache.writeCached(head, { file: { paged: true, of: "v1", size: 20 } });
  await pages.writeBodyPage(head, page("v1", 0, "old-first\n"));
  const scroller = document.createElement("div");
  document.body.append(scroller);
  const paint = vi.fn(() => true);
  view = mountPagedFile(scroller, {
    head,
    file: { paged: true, of: "v1", size: 20 },
    readPage: async (offset) => page("v1", offset, "old-secnd\n"),
    painter: { paint },
  });
  await vi.waitFor(() => expect(paint).toHaveBeenCalled());

  expect(await view.more()).toBe(true);
  expect((await pages.readBodyPages(head, "v1")).complete).toBe(true);
});
