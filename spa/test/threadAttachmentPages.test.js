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
});
