// #557: real cached task comments keep their identity and reply action below
// the measured task page head, and release the header at the end of the card.
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { LONG, SHORT, commentGeometry, modes, mountCommentHeaderPage, parkComment } from "./taskCommentHeaderFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

function expectReplyHeader(geometry) {
  expect(geometry.buttonInHeader, "reply belongs in the comment identity header").toBe(true);
  expect(geometry.icon, "reply is an SVG icon").toBe(true);
  expect(geometry.visibleText, "the icon has no visible Reply label").toBe("");
  expect(geometry.accessibleLabel).toBe("Reply");
  expect(geometry.card.right - geometry.button.right, "reply occupies the right edge of its card").toBeLessThanOrEqual(16);
  expect(geometry.button.left).toBeGreaterThanOrEqual(geometry.header.left);
  expect(geometry.button.right).toBeLessThanOrEqual(geometry.viewport.width);
  expect(geometry.button.top).toBeGreaterThanOrEqual(geometry.header.top - 1);
  expect(geometry.button.bottom).toBeLessThanOrEqual(geometry.header.bottom + 1);
  expect(geometry.documentWidth, "comments must not widen the phone viewport").toBeLessThanOrEqual(geometry.viewport.width + 1);
}

function expectPinned(geometry) {
  expectReplyHeader(geometry);
  expect(Math.abs(geometry.header.top - geometry.pageHead.bottom), "comment header pins below the measured task page head").toBeLessThanOrEqual(1);
  expect(geometry.button.top, "reply remains below the task page head").toBeGreaterThanOrEqual(geometry.pageHead.bottom - 1);
  expect(geometry.button.bottom, "reply remains in the viewport").toBeLessThanOrEqual(geometry.viewport.height);
}

for (const mode of modes) {
  it(`keeps the ${mode.label} ${mode.theme} comment header and reply visible only within their comment`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await mountCommentHeaderPage(page, basePath, mode.theme);
      expect(await page.locator("[data-review-reply]").count(), "only today's eligible comments have reply").toBe(2);
      expect(await page.locator('[data-comment-id="tc-sticky-before"] [data-review-reply]').count()).toBe(0);

      await parkComment(page, SHORT, -80);
      const shortBefore = await commentGeometry(page, SHORT);
      expectReplyHeader(shortBefore);
      expect(shortBefore.row.height, "a short comment keeps a compact card").toBeLessThan(180);
      const bodyOffset = shortBefore.body.top - shortBefore.row.top;
      const headerOffset = shortBefore.header.top - shortBefore.row.top;

      await parkComment(page, LONG, 500);
      const middle = await commentGeometry(page, LONG);
      expect(middle.row.top).toBeLessThan(middle.pageHead.bottom - 400);
      expectPinned(middle);
      await captureLayout(page, `task557-${mode.label}-${mode.theme}-middle.png`);
      await parkComment(page, LONG, 900);
      const later = await commentGeometry(page, LONG);
      expectPinned(later);
      expect(later.body.top - middle.body.top).toBeCloseTo(-400, 0);

      await parkComment(page, LONG, later.row.height - later.header.height / 2);
      const leaving = await commentGeometry(page, LONG);
      expect(leaving.header.top, "header starts leaving when its comment ends").toBeLessThan(leaving.pageHead.bottom - 5);
      expect(leaving.header.bottom, "header stays bounded by its comment").toBeLessThanOrEqual(leaving.row.bottom + 1);

      await parkComment(page, SHORT, -80);
      const shortAfter = await commentGeometry(page, SHORT);
      expect(shortAfter.row.height).toBeCloseTo(shortBefore.row.height, 0);
      expect(shortAfter.body.top - shortAfter.row.top).toBeCloseTo(bodyOffset, 0);
      expect(shortAfter.header.top - shortAfter.row.top).toBeCloseTo(headerOffset, 0);
      const gone = await commentGeometry(page, LONG);
      expect(gone.header.bottom, "the previous comment header leaves with its comment").toBeLessThanOrEqual(gone.row.bottom + 1);
      await captureLayout(page, `task557-${mode.label}-${mode.theme}-short.png`);

      await parkComment(page, SHORT, 100);
      const passed = await commentGeometry(page, LONG);
      expect(passed.header.bottom, "the long comment header is no longer visible").toBeLessThan(passed.pageHead.bottom);
      expect(errors).toEqual([]);
      await page.evaluate(() => window.__stickyPage.dispose());
    }, mode);
  }, 60_000);
}
