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

describe("reading an attachment whole", () => {
  it("asks from where the last piece ended until it holds the whole file", async () => {
    const whole = "0123456789";
    const read = vi.fn(async (offset) => ({
      path: "/store/x.webm", mime: "video/webm", size: whole.length, offset,
      content_b64: b64(whole.slice(offset, offset + 4)),
    }));
    const answer = await bodies.readAttachmentWhole(read);
    expect(read.mock.calls.map(([offset]) => offset)).toEqual([0, 4, 8]);
    expect(atob(answer.content_b64)).toBe(whole);
    expect(answer.size).toBe(10);
    expect(answer.mime).toBe("video/webm");
  });

  it("takes a bridge older than ranges at its word: the whole file in one answer", async () => {
    const read = vi.fn(async () => ({ path: "/store/x.png", mime: "image/png", size: 3, content_b64: b64("abc") }));
    const answer = await bodies.readAttachmentWhole(read);
    expect(read).toHaveBeenCalledTimes(1);
    expect(answer.content_b64).toBe(b64("abc"));
  });

  it("stops on an empty piece rather than asking forever", async () => {
    const read = vi.fn(async (offset) => ({ mime: "video/mp4", size: 100, content_b64: offset ? "" : b64("ab") }));
    const answer = await bodies.readAttachmentWhole(read);
    expect(read).toHaveBeenCalledTimes(2);
    expect(atob(answer.content_b64)).toBe("ab");
  });
});

describe("an issue page's attachment bodies", () => {
  it("writes a fetched body through to the cache and answers the next mount from it", async () => {
    const call = vi.fn(async (_method, params) => ({ path: params.path, mime: "image/png", size: 3, offset: 0, content_b64: b64("png") }));
    const first = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    expect((await first.load("/store/a.png")).content_b64).toBe(b64("png"));
    expect(call).toHaveBeenCalledWith("issues.attachment", { issue_id: "issue-1", path: "/store/a.png" });
    expect((await localCache.readCached(address("/store/a.png")))?.value.content_b64).toBe(b64("png"));
    first.dispose();

    const offline = vi.fn(() => new Promise(() => {}));
    const second = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call: offline });
    expect((await second.load("/store/a.png")).content_b64).toBe(b64("png"));
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
    expect(body.content_b64).toBe(pages.base64Of(whole));
    held.dispose();

    const head = await localCache.readCached(address("/store/long.mp4"));
    expect(head.value).toEqual({ mime: "video/mp4", size, paged: true, of: "whole" });
    const records = await localCache.cachedRecords({ deviceId: "dev-1", entityId: "issue-1", kind: pages.PAGE_RECORD_KIND });
    expect(records.length).toBe(Math.ceil(size / pages.BODY_PAGE_BYTES));
    records.forEach((record) => expect(record.value.end - record.value.offset).toBeLessThanOrEqual(pages.BODY_PAGE_BYTES));

    const offline = vi.fn(() => new Promise(() => {}));
    const revisit = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call: offline });
    expect((await revisit.load("/store/long.mp4")).content_b64).toBe(pages.base64Of(whole));
    expect(offline).not.toHaveBeenCalled();
    revisit.dispose();
  });

  it("still answers a fetched body when this browser has no IndexedDB to keep it in", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    bodies = await import("../src/core/issueAttachmentBodies.js");
    const call = vi.fn(async (_method, params) => ({ path: params.path, mime: "image/png", size: 3, offset: 0, content_b64: b64("png") }));
    const held = bodies.createIssueAttachmentBodies({ deviceId: "dev-1", issueId: "issue-1", call });
    const body = await held.load("/store/a.png");
    expect(body.content_b64).toBe(b64("png"));
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
