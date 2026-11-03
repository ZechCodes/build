import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountVerdictReviewTask, path } from "./taskReviewVerdictsFixture.mjs";

async function assertNoOverflow(page) {
  const size = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth }));
  expect(size.page, JSON.stringify(size)).toBeLessThanOrEqual(size.viewport + 1);
}

async function addDraft(page, side, line, body) {
  const previous = await page.locator("[data-review-draft]").count();
  await page.locator(`[data-review-path="${path}"] [data-review-comment][data-side="${side}"][data-line="${line}"]`).click();
  const composer = page.locator("form[data-review-inline-composer]");
  await composer.locator("textarea").fill(body);
  await composer.locator('[type="submit"]').click();
  await page.waitForFunction((count) => document.querySelectorAll("[data-review-draft]").length === count, previous + 1);
}

for (const { label, width } of [{ label: "desktop", width: 1440 }, { label: "phone", width: 390 }]) {
  it(`composes a batch and confirms a review verdict on ${label}`, async () => {
    const phone = width <= 640;
    await withLayoutPage(async ({ page, basePath }) => {
      await mountVerdictReviewTask(page, basePath);
      const expansion = page.locator(`[data-review-path="${path}"] [data-review-expand]`);
      if (await expansion.getAttribute("aria-expanded") === "false") await expansion.click();
      await page.waitForSelector('[data-review-comment][data-side="new"]');
      await page.locator('[data-review-comment][data-side="new"][data-line="1"]').click();
      const composer = page.locator("form[data-review-inline-composer]");
      await composer.locator("textarea").fill("Explain how this state survives a reload.");
      expect(await composer.isVisible()).toBe(true);
      const composerBox = await composer.boundingBox();
      expect(composerBox.width).toBeLessThanOrEqual(width);
      await assertNoOverflow(page);
      await captureLayout(page, `review-verdict-inline-${label}.png`);
      await composer.locator('[type="submit"]').click();
      await page.waitForFunction(() => document.querySelectorAll("[data-review-draft]").length === 1);
      await addDraft(page, "old", 1, "Keep the previous behavior covered by a regression test.");
      await addDraft(page, "new", 2, "Preserve the reading place when switching commits.");
      if (phone) {
        expect(await page.locator('button[data-review-view="comments"]').textContent()).toContain("3");
        await page.locator('button[data-review-view="comments"]').click();
      }
      await page.waitForFunction(() => document.querySelectorAll("[data-review-draft]").length === 3);
      expect(await page.locator("[data-review-pending]").textContent()).toContain("3");
      await page.locator("[data-review-pending]").scrollIntoViewIfNeeded();
      await assertNoOverflow(page);
      await captureLayout(page, `review-verdict-tray-${label}.png`);
      const requestChanges = page.locator('[data-review-verdict="request_changes"]');
      expect(await requestChanges.isVisible()).toBe(true);
      if (phone) {
        const pin = await requestChanges.evaluate((button) => {
          let node = button;
          while (node && getComputedStyle(node).position !== "fixed") node = node.parentElement;
          return node ? { bottom: node.getBoundingClientRect().bottom, viewport: innerHeight } : null;
        });
        expect(pin).not.toBeNull();
        expect(Math.abs(pin.bottom - pin.viewport)).toBeLessThanOrEqual(1);
      }
      await requestChanges.click();
      const confirmation = page.locator("#confirm-scrim");
      await confirmation.locator("[data-confirm-ok]").waitFor({ state: "visible" });
      await page.evaluate(() => Promise.all(document.querySelector("#confirm-scrim").getAnimations({ subtree: true }).map((animation) => animation.finished.catch(() => {}))));
      expect(await confirmation.isVisible()).toBe(true);
      expect(await confirmation.textContent()).toContain("The author is told and continues");
      await assertNoOverflow(page);
      await captureLayout(page, `review-verdict-confirm-${label}.png`);
      await confirmation.locator("[data-confirm-ok]").click();
      await page.waitForFunction(() => window.__reviewCommentCalls.length === 4);
      await page.waitForFunction(() => document.querySelector('#comment-posted-4 [data-verdict="request_changes"]'));
      const calls = await page.evaluate(() => window.__reviewCommentCalls);
      expect(calls.slice(0, 3).every((params) => params.anchor?.snapshot_id === "revision-2")).toBe(true);
      expect(calls[3].opinion).toEqual({ snapshot_id: "revision-2", verdict: "request_changes" });
      expect(calls[3].body).toContain("[[#970:r2:");
      if (phone) await page.locator('button[data-review-view="comments"]').click();
      await page.locator("#comment-posted-4").scrollIntoViewIfNeeded();
      expect(await page.locator("#comment-posted-4 a.md-ref").count()).toBe(3);
      await assertNoOverflow(page);
      await captureLayout(page, `review-verdict-timeline-${label}.png`);
      await page.evaluate(() => { window.__reviewHeader = document.querySelector(".task-review-header"); });
      await page.locator("#comment-posted-4 a.md-ref").nth(1).click();
      await page.waitForSelector('tr.task-review-anchor[data-old-line="1"]');
      expect(await page.evaluate(() => window.__reviewNavigation.at(-1).review)).toMatchObject({ revision: 2, mode: "all", path, side: "old", line: 1 });
      expect(await page.evaluate(() => document.querySelector(".task-review-header") === window.__reviewHeader)).toBe(true);
      await page.evaluate(() => window.__reviewTaskPage.dispose());
    }, { width, height: 900, plugins: [deviceShim] });
  }, 90_000);
}
