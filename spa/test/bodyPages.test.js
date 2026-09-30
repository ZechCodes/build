// A body too large for one record, kept as pages (#95): each page its own
// record, chained by byte offsets, joined only while they are of one body.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let pages, cache;

const head = { deviceId: "dev-1", entityId: "run-1", kind: "filediff", sub: "big.txt" };

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  pages = await import("../src/core/bodyPages.js");
});

const lines = (count, from = 0) => Array.from({ length: count }, (_, index) => `line ${from + index}\n`).join("");

describe("text pages of a body read whole", () => {
  it("cut the text into pages that each end a line and chain by UTF-8 byte offsets", () => {
    const text = `${lines(40)}é tail\n`;
    const cut = pages.textPagesOf(text, { of: "k1", bytes: 64 });
    expect(cut.length).toBeGreaterThan(3);
    expect(cut.map((page) => page.body).join("")).toBe(text);
    cut.slice(0, -1).forEach((page) => expect(page.body.endsWith("\n")).toBe(true));
    cut.forEach((page, index) => {
      if (index) expect(page.offset).toBe(cut[index - 1].end);
      expect(page.of).toBe("k1");
      expect(page.total).toBe(new TextEncoder().encode(text).length);
    });
    expect(cut.at(-1).end).toBe(new TextEncoder().encode(text).length);
  });

  it("keep only the whole lines before a bridge's cut, and know no total", () => {
    const cut = pages.textPagesOf("one\ntwo\nthr", { of: "k1", cut: true });
    expect(cut).toEqual([{ of: "k1", offset: 0, end: 8, total: null, body: "one\ntwo\n" }]);
  });

  it("answer one empty page for an empty body", () => {
    expect(pages.textPagesOf("", { of: "k1" })).toEqual([{ of: "k1", offset: 0, end: 0, total: 0, body: "" }]);
  });
});

describe("byte pages of a body read whole", () => {
  it("cut the bytes into pages of at most the page size, base64 each, and join back", () => {
    const bytes = Uint8Array.from({ length: 1000 }, (_, index) => index % 251);
    const b64 = pages.base64Of(bytes);
    const cut = pages.bytePagesOf(b64, { of: "a", bytes: 300 });
    expect(cut.map((page) => [page.offset, page.end])).toEqual([[0, 300], [300, 600], [600, 900], [900, 1000]]);
    expect(pages.joinedBase64(cut)).toBe(b64);
  });
});

describe("page records", () => {
  it("releases complete in-memory pages after a media Blob is made, leaving disk pages intact", async () => {
    const cut = pages.bytePagesOf(Buffer.from("abcdef").toString("base64"), { of: "clip", bytes: 2 });
    await pages.writeBodyPages(head, cut);
    const body = pages.createPagedBody({ head, of: "clip" });
    await body.hydrate();
    expect(body.state().pages).toHaveLength(3);
    body.releasePages();
    expect(body.state()).toMatchObject({ pages: [], complete: true, end: 6 });
    await body.hydrate();
    expect(body.state().pages).toHaveLength(0);
    expect((await pages.readBodyPages(head, "clip")).pages).toHaveLength(3);
    body.dispose();
  });

  it("are read back in order from the first, as far as they chain", async () => {
    const cut = pages.textPagesOf(lines(30), { of: "k1", bytes: 40 });
    await pages.writeBodyPages(head, cut);
    const held = await pages.readBodyPages(head, "k1");
    expect(pages.joinedText(held.pages)).toBe(lines(30));
    expect(held.complete).toBe(true);
    expect(held.end).toBe(held.total);
  });

  it("stop at the first missing page and say the body is not complete", async () => {
    const cut = pages.textPagesOf(lines(30), { of: "k1", bytes: 40 });
    await pages.writeBodyPage(head, cut[0]);
    await pages.writeBodyPage(head, cut[2]);
    const held = await pages.readBodyPages(head, "k1");
    expect(held.pages).toEqual([cut[0]]);
    expect(held.end).toBe(cut[0].end);
    expect(held.complete).toBe(false);
  });

  it("stop at the page that reaches the body's end, whatever else of it lies beyond", async () => {
    await pages.writeBodyPages(head, pages.textPagesOf(lines(30), { of: "whole", bytes: 40 }));
    const shorter = pages.textPagesOf(lines(2), { of: "whole", bytes: 40 });
    await pages.writeBodyPage(head, shorter[0]);
    const held = await pages.readBodyPages(head, "whole");
    expect(pages.joinedText(held.pages)).toBe(lines(2));
    expect(held.complete).toBe(true);
  });

  it("are each a record of their own under the page kind", async () => {
    await pages.writeBodyPages(head, pages.textPagesOf(lines(30), { of: "k1", bytes: 40 }));
    const subs = await cache.cachedSubKeys("dev-1", "run-1", pages.PAGE_RECORD_KIND);
    expect(subs.length).toBeGreaterThan(3);
    expect(subs.every((sub) => sub.startsWith("filediff:big.txt@"))).toBe(true);
  });

  it("never join pages of another version, and a new first page lets the old version go", async () => {
    const old = pages.textPagesOf(lines(30), { of: "k1", bytes: 40 });
    await pages.writeBodyPages(head, old);
    expect((await pages.readBodyPages(head, "k2")).pages).toEqual([]);

    const fresh = pages.textPagesOf(lines(3, 100), { of: "k2", bytes: 40 });
    await pages.writeBodyPage(head, fresh[0]);
    const subs = await cache.cachedSubKeys("dev-1", "run-1", pages.PAGE_RECORD_KIND);
    expect(subs).toEqual(["filediff:big.txt@0"]);
    expect((await pages.readBodyPages(head, "k1")).pages).toEqual([]);
  });

  it("are dropped for one body without touching another's", async () => {
    const other = { ...head, sub: "other.txt" };
    await pages.writeBodyPages(head, pages.textPagesOf(lines(5), { of: "k1" }));
    await pages.writeBodyPages(other, pages.textPagesOf(lines(5), { of: "k9" }));
    await pages.dropBodyPages(head);
    expect((await pages.readBodyPages(head, "k1")).pages).toEqual([]);
    expect((await pages.readBodyPages(other, "k9")).complete).toBe(true);
  });

  it("announce their writes to the body's own listeners only", async () => {
    const heard = [];
    const unwatch = pages.subscribeBodyPages(head, (address) => heard.push(address.sub));
    await pages.writeBodyPages({ ...head, sub: "other.txt" }, pages.textPagesOf("x\n", { of: "k" }));
    await pages.writeBodyPages(head, pages.textPagesOf("x\n", { of: "k" }));
    expect(heard).toEqual(["filediff:big.txt@0"]);
    unwatch();
  });
});

describe("a paged body a view holds", () => {
  const bridgeOf = (text, of = "k1", bytes = 40) => {
    const all = pages.textPagesOf(text, { of, bytes });
    return async (offset) => all.find((page) => page.offset === offset) || null;
  };

  it("paints what the cache holds and reads the next page into the cache", async () => {
    const text = lines(30);
    const readPage = vi.fn(bridgeOf(text));
    await pages.writeBodyPage(head, await readPage(0));
    const seen = [];
    const body = pages.createPagedBody({ head, of: "k1", readPage, onChange: (state) => seen.push(state.end) });
    const first = await body.hydrate();
    expect(first.complete).toBe(false);

    expect(await body.more()).toBe(true);
    expect(body.state().pages.length).toBe(2);
    expect(readPage).toHaveBeenLastCalledWith(first.end);
    const stored = await pages.readBodyPages(head, "k1");
    expect(stored.pages.length).toBe(2);

    while (!body.state().complete) await body.more();
    expect(pages.joinedText(body.state().pages)).toBe(text);
    expect(await body.more()).toBe(false);
    body.dispose();
  });

  it("asks once while a page is being read", async () => {
    const readPage = vi.fn(bridgeOf(lines(30)));
    await pages.writeBodyPage(head, await readPage(0));
    readPage.mockClear();
    const body = pages.createPagedBody({ head, of: "k1", readPage });
    await body.hydrate();
    await Promise.all([body.more(), body.more(), body.more()]);
    expect(readPage).toHaveBeenCalledTimes(1);
    body.dispose();
  });

  it("says the body moved rather than join a page of another version", async () => {
    await pages.writeBodyPage(head, (await bridgeOf(lines(30))(0)));
    const moved = [];
    const body = pages.createPagedBody({
      head,
      of: "k1",
      readPage: async (offset) => ({ of: "k2", offset, end: offset + 3, total: 99, body: "new" }),
      onMoved: (page) => moved.push(page.of),
    });
    await body.hydrate();
    expect(await body.more()).toBe(false);
    expect(moved).toEqual(["k2"]);
    expect(body.state().pages.length).toBe(1);
    body.dispose();
  });

  it("cannot read on from a bridge that cannot page", async () => {
    await pages.writeBodyPages(head, pages.textPagesOf("one\ntwo\nthr", { of: "k1", cut: true }));
    const body = pages.createPagedBody({ head, of: "k1", readPage: async () => null });
    const held = await body.hydrate();
    expect(held.complete).toBe(false);
    expect(await body.more()).toBe(false);
    body.dispose();
  });

  it("repaints when another tab writes one of its pages", async () => {
    const readPage = bridgeOf(lines(30));
    await pages.writeBodyPage(head, await readPage(0));
    const seen = [];
    const body = pages.createPagedBody({ head, of: "k1", onChange: (state) => seen.push(state.pages.length) });
    await body.hydrate();
    await pages.writeBodyPage(head, await readPage(body.state().end));
    await vi.waitFor(() => expect(seen.at(-1)).toBe(2));
    body.dispose();
  });
});

describe("a page as a bridge answered it", () => {
  it("takes the page's text from the answer's field and its place from range", () => {
    const answer = { patch: "+a\n", range: { offset: 10, end: 13, total: 40, version: "v1" } };
    expect(pages.pageFromAnswer(answer, "patch")).toEqual({ of: "v1", offset: 10, end: 13, total: 40, body: "+a\n" });
  });
});

describe("paging's edges (#95 review)", () => {
  it("never parts a surrogate pair when a line is longer than a page", () => {
    const text = `${"x".repeat(39)}😀tail\n`;
    const cut = pages.textPagesOf(text, { of: "k", bytes: 40 });
    expect(cut[0].body).toBe("x".repeat(39));
    expect(cut[1].body.startsWith("😀")).toBe(true);
    expect(cut.at(-1).end).toBe(new TextEncoder().encode(text).length);
  });

  it("lets an empty last page complete a body whose size was not known", async () => {
    await pages.writeBodyPages(head, pages.textPagesOf("one\ntwo\nthr", { of: "k1", cut: true }));
    await pages.writeBodyPage(head, { of: "k1", offset: 8, end: 8, total: 8, body: "" });
    const held = await pages.readBodyPages(head, "k1");
    expect(held.complete).toBe(true);
    expect(pages.joinedText(held.pages)).toBe("one\ntwo\n");
  });

  it("reads on from what it held, and reads again from the start when a held page is gone", async () => {
    const cut = pages.textPagesOf(lines(30), { of: "k1", bytes: 40 });
    await pages.writeBodyPages(head, cut.slice(0, 2));
    const first = await pages.readBodyPages(head, "k1");
    await pages.writeBodyPages(head, cut.slice(2));
    const spy = vi.spyOn(cache, "readCachedMany");
    const next = await pages.readBodyPages(head, "k1", first);
    expect(spy.mock.calls[0][0].map((address) => address.sub)).not.toContain("filediff:big.txt@0");
    expect(pages.joinedText(next.pages)).toBe(lines(30));
    spy.mockRestore();

    await cache.deleteCached([{ deviceId: "dev-1", entityId: "run-1", kind: pages.PAGE_RECORD_KIND, sub: "filediff:big.txt@0" }]);
    expect((await pages.readBodyPages(head, "k1", next)).pages).toEqual([]);
  });

  it("does not join a page that starts anywhere but where the held pages end, or that carries nothing", () => {
    expect(pages.pageFollows({ of: "k", offset: 10, end: 20, total: 40 }, 10, "k")).toBe(true);
    expect(pages.pageFollows({ of: "k", offset: 8, end: 20, total: 40 }, 10, "k")).toBe(false);
    expect(pages.pageFollows({ of: "k", offset: 10, end: 10, total: 40 }, 10, "k")).toBe(false);
    expect(pages.pageFollows({ of: "k", offset: 10, end: 10, total: 10 }, 10, "k")).toBe(true);
    expect(pages.pageFollows({ of: "j", offset: 10, end: 20, total: 40 }, 10, "k")).toBe(false);
  });

  it("answers no progress, and keeps nothing, for a page that does not follow on", async () => {
    await pages.writeBodyPage(head, { of: "k1", offset: 0, end: 10, total: 40, body: "0123456789" });
    const body = pages.createPagedBody({ head, of: "k1", readPage: async () => ({ of: "k1", offset: 0, end: 10, total: 40, body: "0123456789" }) });
    await body.hydrate();
    expect(await body.more()).toBe(false);
    body.dispose();
  });

  it("keeps no page for a body the cache no longer holds", async () => {
    await pages.writeBodyPage(head, { of: "k1", offset: 0, end: 10, total: 40, body: "0123456789\n".slice(0, 10) });
    const body = pages.createPagedBody({
      head,
      of: "k1",
      keepGuard: async () => false,
      readPage: async (offset) => ({ of: "k1", offset, end: offset + 5, total: 40, body: "abcd\n" }),
    });
    await body.hydrate();
    expect(await body.more()).toBe(false);
    expect(await cache.cachedSubKeys("dev-1", "run-1", pages.PAGE_RECORD_KIND)).toEqual(["filediff:big.txt@0"]);
    body.dispose();
  });
});
