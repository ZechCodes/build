// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { readCached, writeCached } from "../src/core/localCache.js";
import { createThreadState, threadHtml, wireThreadAttachments, wireThreadSentMessages } from "../src/core/thread.js";

const attachments = [
  { name: "agent-preview.png", path: "/store/agent-preview.png", mime: "image/png", size: 4 },
  { name: "verification.log", path: "/store/verification.log", mime: "text/plain", size: 6 },
];
const otherAgent = { id: "worker-1", owner: { kind: "project", id: "project-1", name: "Build" }, topic: "Attachment evidence" };

afterEach(() => {
  document.querySelector(".thread-lightbox-close")?.click();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("agent message attachments", () => {
  it.each([
    { direction: "agent reply", role: "agent" },
    { direction: "incoming agent message", role: "user", from_agent: otherAgent },
    { direction: "outgoing agent message", role: "agent", sent_to: otherAgent },
  ])("renders cached $direction images and files through the shared lightbox and download wiring", async ({ direction, ...message }) => {
    const address = { deviceId: "attachment-device", entityId: "conversation-1", kind: "thread", sub: direction };
    await writeCached(address, { items: [{ type: "message", data: {
      ...message, id: "message-1", sequence: 1, body: "Review the image and verification log.", attachments,
    } }] });
    const state = createThreadState({ ownerId: direction });
    const render = async () => {
      document.body.innerHTML = threadHtml((await readCached(address)).value, { threadState: state });
      wireThreadSentMessages(document.body, state);
    };
    await render();
    const row = document.querySelector(".thread-message");
    expect(row.querySelector(".thread-attachment-image")?.alt).toBe("agent-preview.png");
    expect(row.querySelector("button.thread-attachment")?.dataset.attachmentName).toBe("verification.log");
    if (message.sent_to) {
      expect(row.querySelector(".thread-body").hidden).toBe(true);
      row.querySelector(".thread-sent-preview").click();
      expect(row.querySelector(".thread-body").hidden).toBe(false);
    }

    const load = vi.fn(async (path) => ({
      mime: path.endsWith(".png") ? "image/png" : "text/plain", content_b64: "AAAA",
    }));
    wireThreadAttachments(document.body, load, state);
    await vi.waitFor(() => expect(row.querySelector(".thread-attachment-image").src).toMatch(/^blob:/));
    row.querySelector(".thread-attachment-preview").click();
    await vi.waitFor(() => expect(document.querySelector(".thread-lightbox img")?.alt).toBe("agent-preview.png"));
    document.querySelector(".thread-lightbox-close").click();

    const download = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    row.querySelector("button.thread-attachment").click();
    await vi.waitFor(() => expect(download).toHaveBeenCalledOnce());
    expect(download.mock.instances[0].download).toBe("verification.log");
    expect(load.mock.calls.map(([path]) => path)).toEqual(attachments.map(({ path }) => path));

    // A cache repaint reuses the held image bytes even with no available wire.
    await render();
    const unavailableWire = vi.fn(async () => { throw new Error("your device went offline"); });
    wireThreadAttachments(document.body, unavailableWire, state);
    expect(document.querySelector(".thread-attachment-image").src).toMatch(/^blob:/);
    expect(unavailableWire).not.toHaveBeenCalled();
    state.dispose();
  });
});
