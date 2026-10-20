import { describe, expect, it, vi } from "vitest";
import { readThreadAttachment, THREAD_ATTACHMENT_PAGE_BYTES } from "../src/core/threadAttachmentPages.js";

describe("thread attachment pages", () => {
  it("reads every announced page without joining a whole-file base64 string", async () => {
    const load = vi.fn(async (_path, offset) => ({
      mime: "video/mp4", size: 6, offset,
      content_b64: Buffer.from("abcdef".slice(offset, offset + 2)).toString("base64"),
    }));
    const body = await readThreadAttachment(load, "clip.mp4", true);
    expect(load.mock.calls.map(([, offset, length]) => [offset, length])).toEqual([
      [0, THREAD_ATTACHMENT_PAGE_BYTES], [2, THREAD_ATTACHMENT_PAGE_BYTES], [4, THREAD_ATTACHMENT_PAGE_BYTES],
    ]);
    expect(body.pages).toEqual(["YWI=", "Y2Q=", "ZWY="]);
  });

  it("keeps the old bridge's one whole-file call", async () => {
    const load = vi.fn(async () => ({ mime: "video/mp4", content_b64: "YWI=" }));
    expect((await readThreadAttachment(load, "clip.mp4", false)).pages).toEqual(["YWI="]);
    expect(load).toHaveBeenCalledWith("clip.mp4");
  });

  it.each([true, false])("reconstructs more than the default 5 MiB cap with paging advertised=%s", async (canPage) => {
    const bytes = Buffer.alloc(5 * 1024 * 1024 + 123, 0xa7);
    bytes[bytes.length - 1] = 0x31;
    const load = vi.fn(async (_path, offset = 0, length = 5 * 1024 * 1024) => ({
      mime: "application/octet-stream", size: bytes.length, offset,
      content_b64: bytes.subarray(offset, offset + length).toString("base64"),
    }));
    const body = await readThreadAttachment(load, "large.bin", canPage);
    const reconstructed = Buffer.concat(body.pages.map((page) => Buffer.from(page, "base64")));
    expect(reconstructed.length).toBe(bytes.length);
    expect(reconstructed.equals(bytes)).toBe(true);
    expect(load.mock.calls[0]).toEqual(canPage ? ["large.bin", 0, THREAD_ATTACHMENT_PAGE_BYTES] : ["large.bin"]);
    expect(load.mock.calls.at(-1)[1]).toBe(5 * 1024 * 1024);
    expect(body.size).toBe(bytes.length);
  });

  it.each([
    { label: "empty first page", first: { content_b64: "" } },
    { label: "wrong first offset", first: { offset: 1 } },
    { label: "extra bytes", first: { size: 1 } },
    { label: "negative size", first: { size: -1 } },
    { label: "noninteger size", first: { size: 1.5 } },
    { label: "wrong path", first: { path: "another.bin" } },
    { label: "empty continuation", next: { content_b64: "" } },
    { label: "repeated offset", next: { offset: 0 } },
    { label: "skipped offset", next: { offset: 3 } },
    { label: "missing offset", next: { offset: undefined } },
    { label: "changed size", next: { size: 5 } },
    { label: "missing size", next: { size: undefined } },
    { label: "changed MIME", next: { mime: "image/png" } },
  ])("refuses an incomplete or inconsistent attachment: $label", async ({ first, next }) => {
    const load = vi.fn().mockResolvedValueOnce({
      path: "data.bin", mime: "application/octet-stream", size: 4, offset: 0, content_b64: "YWI=", ...first,
    }).mockResolvedValueOnce({
      path: "data.bin", mime: "application/octet-stream", size: 4, offset: 2, content_b64: "Y2Q=", ...next,
    });
    await expect(readThreadAttachment(load, "data.bin", true)).rejects.toThrow(/attachment/i);
    expect(load.mock.calls.length).toBeLessThanOrEqual(2);
  });

  it("refuses a capped legacy response when the bridge ignores continuation ranges", async () => {
    const load = vi.fn(async () => ({ mime: "text/plain", size: 6, content_b64: "YWI=" }));
    await expect(readThreadAttachment(load, "log.txt", false)).rejects.toThrow(/attachment/i);
    expect(load).toHaveBeenCalledTimes(2);
  });
});
