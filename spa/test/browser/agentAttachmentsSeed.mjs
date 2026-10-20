import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

/** A cached conversation and held media bodies; no bridge or greeting. */
export async function mountAgentAttachments(page, basePath) {
  const image = await readFile(new URL("../../../design/tasks-dashboard/tasks111-desktop-dark.png", import.meta.url));
  await mountLayout(page, '<main id="attachment-conversation"><header>Build · Agent attachment review</header><div id="attachment-thread"></div></main>', {
    basePath,
    styles: `body{display:block;margin:0;overflow:auto}
      #attachment-conversation{max-width:640px;margin:32px auto;padding:16px}
      #attachment-conversation>header{font-size:17px;font-weight:600;margin-bottom:24px}
      #attachment-thread{width:100%}
      @media(max-width:600px){#attachment-conversation{margin:0;padding:16px 12px}}`,
  });
  await loadBrowserModules(page, {
    thread: "src/core/thread.js", cache: "src/core/localCache.js", media: "src/core/mediaBlob.js",
  }, basePath);
  await page.evaluate(async ({ image, size }) => {
    document.documentElement.dataset.theme = "dark";
    const { thread, cache, media } = window.__layoutModules;
    const attachments = [
      { path: "/store/review-preview.png", name: "review-preview.png", mime: "image/png", size },
      { path: "/store/verification.log", name: "verification.log", mime: "text/plain", size: 30 },
    ];
    const recipient = { id: "review-agent", owner: { kind: "project", id: "build", name: "Build" }, topic: "Review evidence" };
    const items = [
      { type: "message", data: { id: "user-1", sequence: 1, role: "user", body: "Show me the updated interface and the verification log." } },
      { type: "message", data: { id: "agent-2", sequence: 2, role: "agent", body: "The updated interface is ready for review. The screenshot and verification log are attached.", attachments } },
      { type: "message", data: { id: "sent-3", sequence: 3, role: "agent", sent_to: recipient, body: "Please review the screenshot and verification log.", attachments } },
    ];
    const address = { deviceId: "evidence-device", entityId: "evidence-conversation", kind: "thread", sub: "evidence-agent" };
    await cache.writeCached(address, { items });
    const state = thread.createThreadState({ ownerId: "evidence-conversation" });
    for (const attachment of attachments) {
      const body = attachment.mime === "image/png" ? image : btoa("Focused attachment tests pass\n");
      state.rememberAttachment(attachment.path, {
        ...attachment, body: media.createMediaBody([body], attachment.mime),
      });
    }
    const root = document.querySelector("#attachment-thread");
    root.innerHTML = thread.threadHtml((await cache.readCached(address)).value, { threadState: state });
    window.__unexpectedAttachmentReads = [];
    thread.wireThreadAttachments(root, async (path) => {
      window.__unexpectedAttachmentReads.push(path);
      throw new Error("No bridge in this cached fixture");
    }, state);
    thread.wireThreadSentMessages(root, state);
  }, { image: image.toString("base64"), size: image.length });
  await page.waitForFunction(() => [...document.querySelectorAll(".thread-attachment-image")]
    .every((image) => image.complete && image.naturalWidth > 0));
}
