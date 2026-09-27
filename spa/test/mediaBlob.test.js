// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import * as mediaBlob from "../src/core/mediaBlob.js";
import { attachMediaSource, releaseMediaSource } from "../src/core/mediaBlob.js";
import { openAttachmentLightbox } from "../src/core/threadAttachmentLightbox.js";

const originalCreate = URL.createObjectURL;
const originalRevoke = URL.revokeObjectURL;

afterEach(() => {
  URL.createObjectURL = originalCreate;
  URL.revokeObjectURL = originalRevoke;
  document.body.replaceChildren();
});

describe("cached media Blob URLs", () => {
  it("decodes each page into Blob bytes and releases a removed element", async () => {
    const create = vi.fn(() => "blob:clip");
    const revoke = vi.fn();
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
    const video = document.createElement("video");
    document.body.append(video);
    attachMediaSource(video, ["YWJj", "ZGVm"], "video/mp4");

    expect(video.getAttribute("src")).toBe("blob:clip");
    const blob = create.mock.calls[0][0];
    expect(blob.type).toBe("video/mp4");
    expect(await blob.text()).toBe("abcdef");
    video.remove();
    await vi.waitFor(() => expect(revoke).toHaveBeenCalledWith("blob:clip"));
  });

  it("releases the previous URL when a media element receives new pages", () => {
    URL.createObjectURL = vi.fn().mockReturnValueOnce("blob:first").mockReturnValueOnce("blob:second");
    URL.revokeObjectURL = vi.fn();
    const image = document.createElement("img");
    document.body.append(image);
    attachMediaSource(image, ["YQ=="], "image/png");
    attachMediaSource(image, ["Yg=="], "image/png");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:first");
    releaseMediaSource(image);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:second");
  });

  it("reuses one incrementally assembled Blob and drops its source pages", async () => {
    const OriginalBlob = globalThis.Blob;
    const parts = [];
    globalThis.Blob = class extends OriginalBlob {
      constructor(chunks, options) {
        parts.push(chunks);
        super(chunks, options);
      }
    };
    try {
      const create = vi.spyOn(URL, "createObjectURL");
      const pages = ["YWI=", "Y2Q=", "ZWY="];
      const body = mediaBlob.createMediaBody(pages, "video/webm");
      const tile = document.createElement("video");
      const lightbox = document.createElement("video");
      document.body.append(tile, lightbox);
      attachMediaSource(tile, body);
      attachMediaSource(lightbox, body);

      expect(create).toHaveBeenCalledTimes(2);
      expect(create.mock.calls[0][0]).toBe(create.mock.calls[1][0]);
      expect(await create.mock.calls[0][0].text()).toBe("abcdef");
      expect(parts).toHaveLength(3);
      expect(parts.map((step) => step.length)).toEqual([1, 2, 2]);
      expect(parts[1][0]).toBeInstanceOf(OriginalBlob);
      expect(pages).toHaveLength(0);
      expect(body.pages).toBeNull();
      create.mockRestore();
    } finally {
      globalThis.Blob = OriginalBlob;
    }
  });

  it("does not create a URL when a slow attachment finishes after close", async () => {
    const create = vi.spyOn(URL, "createObjectURL");
    let finish;
    const trigger = document.createElement("button");
    document.body.append(trigger);
    const modal = openAttachmentLightbox([{
      trigger, path: "late.webm", kind: "video", name: "late.webm",
      source: () => new Promise((resolve) => { finish = resolve; }),
    }]);
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    modal.close();
    finish({ mime: "video/webm", pages: ["YQ=="] });
    await new Promise((resolve) => setTimeout(resolve, 0));
    modal.show(0);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(create).not.toHaveBeenCalled();
    create.mockRestore();
  });
});
