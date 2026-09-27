// #153: typing into the task page's comment box moved the page's scroll. The
// real page is mounted from the cache in Chromium, scrolled so the box is in
// view, and typed into one key at a time, each one waiting out the draft's
// write so its echo through the cache has painted.

import { expect, it } from "vitest";
import answer from "../../../fixtures/api/v1/tasks.get.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const SHELL = '<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>';
const STYLES = (basePath) => `@import url("${basePath}src/styles/tasks.css");
  @import url("${basePath}src/styles/surfaces.css");
  html, body { height:100%; margin:0; } body { display:block; } #tabbody { height:100%; }`;

/** A task long enough that the comment box sits a long scroll down. */
const longTask = () => {
  const task = structuredClone(answer.result.task);
  const timeline = Array.from({ length: 24 }, (_, at) => ({
    type: "comment",
    id: `tc-${String(at).padStart(4, "0")}`,
    author: { kind: "user" },
    body: `Comment ${at}.\n\nA second paragraph so each row takes some room on a phone and a desk.`,
    created_at: `2026-09-19T10:${String(at).padStart(2, "0")}:00Z`,
    // Screenshots, the way #146's comments carry them: their bytes are a
    // round trip away, so a page that rebuilt its rows would lose their height.
    attachments: at % 3 ? [] : [{ name: `shot-${at}.png`, path: `/store/shot-${at}.png`, mime: "image/png", size: 1 }],
  }));
  return { task, timeline };
};

const scrollerTops = (page) => page.evaluate(() => ({
  pane: document.querySelector("#task-pane").scrollTop,
  document: document.scrollingElement.scrollTop,
}));

for (const [label, width, height] of [["phone", 390, 844], ["desktop", 1440, 900]]) {
  it(`keeps the ${label} scroll still while typing into the comment box`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await mountLayout(page, SHELL, { basePath, styles: STYLES(basePath) });
      await loadBrowserModules(page, { cache: "src/core/trackerCache.js", taskPage: "src/core/trackerTaskPage.js" }, basePath);
      await page.evaluate(async (record) => {
        const { cache, taskPage } = window.__layoutModules;
        await cache.writeTasksRecord("dev-1", "proj-1", { tasks: [record.task], columns: [] });
        await cache.writeTaskRecord("dev-1", "proj-1", record.task.id, record);
        window.__typingPage = taskPage.mountTaskPage(document.querySelector("#task-pane"), {
          deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1|proj-1", taskId: record.task.id,
          feed: () => ({ projects: [], workspaces: [], items: [] }),
          // The bridge answers only for the screenshots' bytes; the page itself
          // paints from the cache alone.
          callRpc: async (method) => {
            if (method !== "tasks.attachment") return new Promise(() => {});
            const canvas = new OffscreenCanvas(390, 700);
            const context = canvas.getContext("2d");
            context.fillStyle = "#51ffb4";
            context.fillRect(0, 0, 390, 700);
            const bytes = new Uint8Array(await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer());
            return { mime: "image/png", size: bytes.length, offset: 0, content_b64: btoa(String.fromCharCode(...bytes)) };
          },
          catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }), navigate: () => {},
        });
      }, longTask());

      const field = page.locator("#task-comment");
      await field.waitFor();
      // Every screenshot has its bytes and its height before the scroll is read.
      await page.waitForFunction(() => {
        const shots = [...document.querySelectorAll("img.thread-attachment-image")];
        return shots.length === 8 && shots.every((shot) => shot.complete && shot.naturalHeight > 0);
      });
      // Park the box in the middle of the pane, the way a reader scrolls to it.
      await page.evaluate(() => {
        const pane = document.querySelector("#task-pane");
        const box = document.querySelector("#task-comment");
        pane.scrollTop += box.getBoundingClientRect().top - pane.getBoundingClientRect().top - pane.clientHeight / 3;
      });
      await field.focus();
      // The scroll above announces itself a frame later; let it, before listening.
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      const node = await field.elementHandle();
      // A jump the page later scrolls back from is still a jump: every scroll
      // event while typing is recorded, not just where the pane settles.
      await page.evaluate(() => {
        window.__scrolled = [];
        const pane = document.querySelector("#task-pane");
        pane.addEventListener("scroll", () => window.__scrolled.push(pane.scrollTop));
      });
      const before = await scrollerTops(page);
      await captureLayout(page, `task-comment-typing-${label}-1-before.png`);
      expect(before.pane, "the pane has to be scrolled for the check to mean anything").toBeGreaterThan(200);

      for (const key of "abc") {
        await page.keyboard.type(key);
        await page.waitForTimeout(400); // past the 180 ms draft debounce and its echo
        const after = await scrollerTops(page);
        expect(after, `after typing ${key}`).toEqual(before);
        expect(await page.evaluate(() => window.__scrolled), `scrolls while typing ${key}`).toEqual([]);
        expect(await node.evaluate((element) => element.isConnected && document.activeElement === element)).toBe(true);
      }
      expect(await node.evaluate((element) => [element.value, element.selectionStart])).toEqual(["abc", 3]);
      await captureLayout(page, `task-comment-typing-${label}-2-typed.png`);

      // An agent comments while the reader is mid-sentence. The row lands
      // above the box; the box stays where it is on screen, with its caret.
      const boxTop = () => node.evaluate((element) => element.getBoundingClientRect().top);
      const top = await boxTop();
      await page.evaluate(async (record) => {
        record.timeline.push({
          type: "comment", id: "tc-pushed", author: { kind: "user" }, body: "Pushed while typing.",
          created_at: "2026-09-19T11:00:00Z",
          attachments: [{ name: "shot-pushed.png", path: "/store/shot-pushed.png", mime: "image/png", size: 1 }],
        });
        await window.__layoutModules.cache.writeTaskRecord("dev-1", "proj-1", record.task.id, record);
      }, longTask());
      await page.waitForFunction(() => document.querySelector("#comment-tc-pushed img")?.naturalHeight > 0);
      await page.waitForTimeout(200);
      expect(await boxTop(), "the box moved under the reader").toBeCloseTo(top, 0);
      await captureLayout(page, `task-comment-typing-${label}-3-pushed.png`);
      expect(await node.evaluate((element) => [element.isConnected, document.activeElement === element,
        element.value, element.selectionStart])).toEqual([true, true, "abc", 3]);
      expect(errors).toEqual([]);
      await page.evaluate(() => window.__typingPage.dispose());
    }, { width, height, plugins: [deviceShim] });
  }, 30_000);
}
