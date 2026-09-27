// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { writeBodyPage, writeBodyPages, bytePagesOf, readBodyPages } from "../src/core/bodyPages.js";
import { writeCached } from "../src/core/localCache.js";
import { attachMediaSource, createMediaBody } from "../src/core/mediaBlob.js";
import { mountPagedFile } from "../src/core/pagedFileView.js";

it("drops Files media pages after Blob creation and does not rehydrate them on a page announcement", async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  const head = { deviceId: "dev-media", entityId: "run-media", kind: "file", sub: "clip.webm" };
  const pages = bytePagesOf(Buffer.from("abcdef").toString("base64"), { of: "clip", bytes: 2 });
  await writeCached(head, { file: { paged: true, of: "clip", size: 6 } });
  await writeBodyPages(head, pages);
  const scroller = document.createElement("div");
  document.body.append(scroller);
  const paint = vi.fn((content, state) => {
    const video = document.createElement("video");
    content.replaceChildren(video);
    attachMediaSource(video, createMediaBody(state.pages, "video/webm"));
    return true;
  });
  const view = mountPagedFile(scroller, {
    head, file: { of: "clip", size: 6 }, painter: { paint }, releaseCompletedPages: true,
  });
  await vi.waitFor(() => expect(paint).toHaveBeenCalledTimes(1));
  expect(scroller.querySelector("video").getAttribute("src")).toMatch(/^blob:/);
  await writeBodyPage(head, pages[1]);
  await new Promise((resolve) => setTimeout(resolve, 10));
  expect(paint).toHaveBeenCalledTimes(1);
  expect((await readBodyPages(head, "clip")).pages).toHaveLength(3);
  view.dispose();
  document.body.replaceChildren();
});
