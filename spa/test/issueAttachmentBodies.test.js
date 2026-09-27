/** @vitest-environment jsdom */
// An issue attachment's bytes (#116): read back in pieces, written through to
// the cache, and — past the cap — kept in page records (#95).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let bodies, localCache, thresholds;

const b64 = (text) => btoa(text);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  bodies = await import("../src/core/issueAttachmentBodies.js");
  localCache = await import("../src/core/localCache.js");
  thresholds = await import("../src/core/cacheThresholds.js");
});

const address = (path) => ({ deviceId: "dev-1", entityId: "issue-1", kind: thresholds.ATTACHMENT_RECORD_KIND, sub: path });

describe("reading attachment byte pages", () => {
  it("asks from where the last piece ended until it holds the whole file", async () => {
    const whole = "0123456789";
    const read = vi.fn(async (offset) => ({
      path: "/store/x.webm", mime: "video/webm", size: whole.length, offset,
      content_b64: b64(whole.slice(offset, offset + 4)),
    }));
    const answer = await bodies.readAttachmentPages(read);
    expect(read.mock.calls.map(([offset]) => offset)).toEqual([0, 4, 8]);
    expect(answer.pages).toEqual([b64("0123"), b64("4567"), b64("89")]);
    expect(answer.size).toBe(10);
    expect(answer.mime).toBe("video/webm");
  });

  it("takes a bridge older than ranges at its word: the whole file in one answer", async () => {
    const read = vi.fn(async () => ({ path: "/store/x.png", mime: "image/png", size: 3, content_b64: b64("abc") }));
    const answer = await bodies.readAttachmentPages(read);
    expect(read).toHaveBeenCalledTimes(1);
    expect(answer.pages).toEqual([b64("abc")]);
  });

  it("rejects a short download so a partial video cannot play", async () => {
    const read = vi.fn(async (offset) => ({ mime: "video/mp4", size: 100, content_b64: offset ? "" : b64("ab") }));
    await expect(bodies.readAttachmentPages(read)).rejects.toThrow("before the file was complete");
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("rejects a range at the wrong offset", async () => {
    const read = vi.fn(async (offset) => ({ mime: "video/mp4", size: 4, offset: offset ? 1 : 0, content_b64: b64("ab") }));
    await expect(bodies.readAttachmentPages(read)).rejects.toThrow("wrong offset");
  });
});

describe("an issue page's attachment bodies", () => {
  it("returns distinct byte pages and gives every range background priority", async () => {
    const whole = "0123456789";
    const call = vi.fn(async (_method, params) => {
      const offset = params.offset || 0;
      return { mime: "video/webm", size: whole.length, offset, content_b64: b64(whole.slice(offset, offset + 4)) };
    });
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    expect((await held.load("/store/a.webm")).pages).toEqual([b64("0123"), b64("4567"), b64("89")]);
    expect(call.mock.calls.map(([, params]) => params.offset || 0)).toEqual([0, 4, 8]);
    expect(call.mock.calls.every(([, , envelope]) => envelope?.priority === "background")).toBe(true);
    held.dispose();
  });

  it("writes a fetched body through to the cache and answers the next mount from it", async () => {
    const call = vi.fn(async (_method, params) => ({ path: params.path, mime: "image/png", size: 3, offset: 0, content_b64: b64("png") }));
    const first = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    expect((await first.load("/store/a.png")).pages).toEqual([b64("png")]);
    expect(call).toHaveBeenCalledWith("issues.attachment", { issue_id: "issue-1", path: "/store/a.png", length: 262144 }, { priority: "background" });
    expect((await localCache.readCached(address("/store/a.png")))?.value.paged).toBe(true);
    first.dispose();

    const offline = vi.fn(() => new Promise(() => {}));
    const second = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call: offline });
    expect((await second.load("/store/a.png")).pages).toEqual([b64("png")]);
    expect(offline).not.toHaveBeenCalled();
    second.dispose();
  });

  it("keeps a body over the cap in page records and paints it from them (#95)", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const size = thresholds.ATTACHMENT_BODY_MAX_BYTES + 1;
    const whole = new Uint8Array(size).map((_, index) => index % 253);
    const call = vi.fn(async (_method, params) => {
      const offset = params.offset || 0;
      const piece = whole.subarray(offset, offset + 2 * 1_048_576);
      return { path: params.path, mime: "video/mp4", size, offset, content_b64: pages.base64Of(piece) };
    });
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    const body = await held.load("/store/long.mp4");
    expect(body.mime).toBe("video/mp4");
    expect(body.pages.reduce((size, page) => size + atob(page).length, 0)).toBe(size);
    held.dispose();

    const head = await localCache.readCached(address("/store/long.mp4"));
    expect(head.value).toEqual({ mime: "video/mp4", size, paged: true, of: "whole" });
    const records = await localCache.cachedRecords({ deviceId: "dev-1", entityId: "issue-1", kind: pages.PAGE_RECORD_KIND });
    expect(records.length).toBe(Math.ceil(size / pages.BODY_PAGE_BYTES));
    records.forEach((record) => expect(record.value.end - record.value.offset).toBeLessThanOrEqual(pages.BODY_PAGE_BYTES));

    const offline = vi.fn(() => new Promise(() => {}));
    const revisit = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call: offline });
    expect((await revisit.load("/store/long.mp4")).pages).toEqual(body.pages);
    expect(offline).not.toHaveBeenCalled();
    revisit.dispose();
  });

  it("splits an older bridge's whole answer into byte pages and never asks a second range", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const size = pages.BODY_PAGE_BYTES + 3;
    const whole = new Uint8Array(size).fill(65);
    const call = vi.fn(async () => ({ mime: "video/mp4", size, content_b64: pages.base64Of(whole) }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    const answer = await held.load("/store/old.mp4");
    expect(call).toHaveBeenCalledTimes(1);
    expect(answer.pages.map((part) => atob(part).length)).toEqual([pages.BODY_PAGE_BYTES, 3]);
    held.dispose();
  });

  it("re-fetches a partial body left by an older cache instead of playing it", async () => {
    await localCache.writeCached(address("/store/partial.mp4"), { mime: "video/mp4", size: 4, content_b64: b64("ab") });
    const call = vi.fn(async () => ({ mime: "video/mp4", size: 4, content_b64: b64("abcd") }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    expect((await held.load("/store/partial.mp4")).pages).toEqual([b64("abcd")]);
    expect(call).toHaveBeenCalledTimes(1);
    held.dispose();
  });

  it("keeps the newest two large videos per issue and evicts their pages with old heads", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const size = thresholds.ATTACHMENT_BODY_MAX_BYTES + 1;
    const content_b64 = pages.base64Of(new Uint8Array(size).fill(88));
    const call = vi.fn(async () => ({ mime: "video/mp4", size, content_b64 }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    for (const name of ["a", "b", "c"]) await held.load(`/store/${name}.mp4`);
    expect(await localCache.readCached(address("/store/a.mp4"))).toBeUndefined();
    expect(await localCache.readCached(address("/store/b.mp4"))).toBeDefined();
    expect(await localCache.readCached(address("/store/c.mp4"))).toBeDefined();
    const stored = await localCache.cachedRecords({ deviceId: "dev-1", entityId: "issue-1", kind: pages.PAGE_RECORD_KIND });
    expect(stored.some((record) => record.address.sub.startsWith("attachment:/store/a.mp4@"))).toBe(false);
    held.dispose();
  });

  it("expires large video pages after 72 hours when the issue is opened again", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const size = thresholds.ATTACHMENT_BODY_MAX_BYTES + 1;
    const content_b64 = pages.base64Of(new Uint8Array(size).fill(88));
    const call = vi.fn(async () => ({ mime: "video/mp4", size, content_b64 }));
    const first = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    await first.load("/store/stale.mp4");
    first.dispose();
    const originalNow = Date.now;
    vi.spyOn(Date, "now").mockImplementation(() => originalNow() + 73 * 60 * 60 * 1000);
    const next = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    await next.load("/store/current.png");
    expect(await localCache.readCached(address("/store/stale.mp4"))).toBeUndefined();
    const stored = await localCache.cachedRecords({ deviceId: "dev-1", entityId: "issue-1", kind: pages.PAGE_RECORD_KIND });
    expect(stored.some((record) => record.address.sub.startsWith("attachment:/store/stale.mp4@"))).toBe(false);
    next.dispose();
    vi.restoreAllMocks();
  });

  it("caps large video bodies across issues on one device and removes their pages", async () => {
    const pages = await import("../src/core/bodyPages.js");
    const size = thresholds.ATTACHMENT_BODY_MAX_BYTES + 1;
    for (let index = 0; index < 5; index += 1) {
      const head = { deviceId: "dev-1", entityId: `issue-${index}`, kind: thresholds.ATTACHMENT_RECORD_KIND, sub: `/store/${index}.mp4` };
      await pages.writeBodyPage(head, { of: "whole", offset: 0, end: 1, total: size, body: b64("x") });
      await localCache.writeCached(head, { mime: "video/mp4", size, paged: true, of: "whole" });
    }
    const call = vi.fn(async () => ({ mime: "image/png", size: 1, content_b64: b64("x") }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-5", call });
    await held.load("/store/current.png");
    const heads = (await localCache.cachedAddresses({ deviceId: "dev-1" }))
      .filter((entry) => entry.kind === thresholds.ATTACHMENT_RECORD_KIND && entry.sub.endsWith(".mp4"));
    expect(heads).toHaveLength(4);
    const evicted = Array.from({ length: 5 }, (_, index) => index)
      .find((index) => !heads.some((head) => head.entityId === `issue-${index}`));
    const leftovers = await localCache.cachedRecords({ deviceId: "dev-1", entityId: `issue-${evicted}`, kind: pages.PAGE_RECORD_KIND });
    expect(leftovers).toEqual([]);
    held.dispose();
  });

  it("still answers a fetched body when this browser has no IndexedDB to keep it in", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    bodies = await import("../src/core/issueAttachmentBodies.js");
    const call = vi.fn(async (_method, params) => ({ path: params.path, mime: "image/png", size: 3, offset: 0, content_b64: b64("png") }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    const body = await held.load("/store/a.png");
    expect(body.pages).toEqual([b64("png")]);
    expect(body.mime).toBe("image/png");
    held.dispose();
  });

  it("refuses with the bridge's own error, so the tile can tell a refusal from a dead wire", async () => {
    const call = vi.fn(async () => {
      throw new Error("not an attachment on this issue: /etc/passwd");
    });
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    await expect(held.load("/etc/passwd")).rejects.toThrow("not an attachment on this issue");
    held.dispose();
  });
});
