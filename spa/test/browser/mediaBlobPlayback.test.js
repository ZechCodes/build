import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

// A checked-in VP8 recording, so the browser check does not need ffmpeg or
// network media. Split before crossing into Chromium as the wire does.
const recording = await readFile(fileURLToPath(new URL("../../../design/landing-review/devices-2025/desktop-animatic.webm", import.meta.url)));
const pageBytes = 256 * 1024;
const pages = [];
for (let offset = 0; offset < recording.length; offset += pageBytes) {
  const piece = recording.subarray(offset, offset + pageBytes);
  pages.push({ of: "sample-v1", offset, end: offset + piece.length, total: recording.length, body: piece.toString("base64") });
}

async function assertPlayback(page, selector) {
  await page.waitForFunction((wanted) => {
    const video = document.querySelector(wanted);
    return video?.src.startsWith("blob:") && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA;
  }, selector, { timeout: 30_000 });
  const result = await page.locator(selector).evaluate(async (video) => {
    const source = video.src;
    const bytes = (await (await fetch(source)).arrayBuffer()).byteLength;
    video.muted = true;
    await video.play();
    return { source, bytes, duration: video.duration, playing: !video.paused };
  });
  expect(result.source).toMatch(/^blob:/);
  expect(result.bytes).toBe(recording.length);
  expect(result.duration).toBeGreaterThan(1);
  expect(result.playing).toBe(true);
  await page.waitForFunction((wanted) => document.querySelector(wanted)?.currentTime > 0.1, selector);
}

it("plays cached byte pages through the production lightbox and Files viewer", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<button id="open-clip">Open recording</button><main id="files"></main>', { basePath });
    await loadBrowserModules(page, {
      bodyPages: "src/core/bodyPages.js",
      cache: "src/core/localCache.js",
      lightbox: "src/core/threadAttachmentLightbox.js",
      files: "src/views/files.js",
      scope: "src/core/cacheScope.js",
    }, basePath);
    await page.evaluate(async (cachedPages) => {
      const { bodyPages, cache, lightbox, files, scope } = window.__layoutModules;
      const deviceId = "playback-device";
      const entityId = "playback-run";
      const head = { deviceId, entityId, kind: "file", sub: "recording.webm" };
      await bodyPages.writeBodyPages(head, cachedPages);
      await cache.writeCached(head, {
        file: { path: "recording.webm", mime: "video/webm", size: cachedPages.at(-1).total,
          truncated: false, paged: true, of: "sample-v1", editable: false },
        openedAt: Date.now(),
      });
      await cache.writeCached({ deviceId, entityId, kind: "tree", sub: "" }, {
        path: "", entries: [{ name: "recording.webm", kind: "file", size: cachedPages.at(-1).total }],
      });
      window.__playbackFiles = files.renderFilesTab(document.querySelector("#files"), {
        scope: { run_id: entityId }, cacheScope: scope.scopeFor(deviceId),
        callRpc: async (method) => { throw new Error(`unexpected ${method}`); },
      });
      document.querySelector("#open-clip").onclick = () => lightbox.openAttachmentLightbox([{
        trigger: document.querySelector("#open-clip"), path: "recording.webm", name: "recording.webm", kind: "video",
        source: async () => {
          const held = await bodyPages.readBodyPages(head, "sample-v1");
          if (!held.complete) throw new Error("recording pages incomplete");
          return { pages: held.pages, mime: "video/webm" };
        },
      }]);
    }, pages);

    await page.locator("#open-clip").click();
    await assertPlayback(page, ".thread-lightbox-stage video");
    await page.locator(".thread-lightbox-close").click();
    await page.waitForFunction(() => !document.querySelector(".thread-lightbox"));

    await page.locator('.frow[data-path="recording.webm"]').dblclick();
    await assertPlayback(page, "video.fmedia");
    await page.evaluate(() => window.__playbackFiles.dispose());
  }, { width: 1200, height: 800 });
}, 90_000);
