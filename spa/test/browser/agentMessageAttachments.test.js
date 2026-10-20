import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountAgentAttachments } from "./agentAttachmentsSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

it.each([
  { label: "desktop", width: 1024, height: 900 },
  { label: "mobile", width: 390, height: 844 },
])("opens agent attachment thumbnails and downloads files from cache on $label", async ({ label, width, height }) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountAgentAttachments(page, basePath);
    const reply = page.locator(".thread-comment.agent");
    expect(await reply.locator("img.thread-attachment-image").isVisible()).toBe(true);
    expect(await reply.getByTitle("Download verification.log").isVisible()).toBe(true);
    await captureLayout(page, `task696-agent-attachments-${label}.png`);

    await reply.getByRole("button", { name: "Open review-preview.png" }).click();
    await page.waitForFunction(() => {
      const image = document.querySelector(".thread-lightbox img");
      return image?.complete && image.naturalWidth > 0;
    });
    await captureLayout(page, `task696-agent-attachments-lightbox-${label}.png`);
    await page.keyboard.press("Escape");
    await page.locator(".thread-lightbox").waitFor({ state: "detached" });

    const downloaded = page.waitForEvent("download");
    await reply.getByTitle("Download verification.log").click();
    expect((await downloaded).suggestedFilename()).toBe("verification.log");

    const sent = page.locator(".thread-sent");
    expect(await sent.locator(".thread-attachments").isVisible()).toBe(false);
    await sent.locator(".thread-sent-preview").click();
    expect(await sent.locator("img.thread-attachment-image").isVisible()).toBe(true);
    expect(await sent.getByTitle("Download verification.log").isVisible()).toBe(true);
    expect(await page.evaluate(() => window.__unexpectedAttachmentReads)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await captureLayout(page, `task696-agent-attachments-sent-${label}.png`);
  }, { width, height });
}, 30_000);
