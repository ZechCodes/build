// #475 review screenshots: mount the browser gate's cold cached inbox first,
// then adopt an isolated fake session to show the ordinary online rail.
// From spa/: nice -n 10 node test/browser/watchedTaskInboxCapture.mjs
import { mkdir } from "node:fs/promises";
import assert from "node:assert/strict";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { withLayoutPage } from "./layoutHarness.mjs";
import { QUIET_TASK, adoptWatchedTaskSession, disposeWatchedTaskInbox, mountWatchedTaskInbox } from "./watchedTaskInboxSeed.mjs";

const output = process.env.CAPTURE_DIR || fileURLToPath(new URL("../../../design/inbox-watched-tasks/", import.meta.url));

async function assertVisibleWatchControls(page, state) {
  const controls = await page.locator("#inbox-list .inbox-watch").evaluateAll((buttons) => buttons.map((button) => {
    const bounds = button.getBoundingClientRect();
    const svg = button.querySelector("svg");
    const icon = svg.getBoundingClientRect();
    return { label: button.getAttribute("aria-label"), opacity: getComputedStyle(button).opacity,
      parentOpacity: getComputedStyle(button.parentElement).opacity,
      pointerEvents: getComputedStyle(button).pointerEvents,
      width: bounds.width, height: bounds.height, iconWidth: icon.width, iconHeight: icon.height,
      hit: document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)?.closest("button") === button };
  }));
  assert.equal(controls.length, 3, `${state}: all task rows carry an eye`);
  for (const control of controls) {
    const note = `${state}: ${control.label}: ${JSON.stringify(control)}`;
    assert.equal(control.opacity, "1", note);
    assert.equal(control.parentOpacity, "1", note);
    assert.equal(control.pointerEvents, "auto", note);
    assert.equal(control.hit, true, note);
    assert.equal(control.iconWidth, 17, note);
    assert.equal(control.iconHeight, 17, note);
  }
  console.log(`${state}: ${JSON.stringify(controls)}`);
}

await mkdir(output, { recursive: true });
for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 440, hasTouch: false },
  { label: "phone", width: 390, height: 760, hasTouch: true },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWatchedTaskInbox(page, basePath);
    await page.locator(`[data-key="tracker_task:${QUIET_TASK}"] .inbox-watch`).waitFor({ state: "visible" });
    await page.mouse.move(width - 1, height - 1);
    await assertVisibleWatchControls(page, `${label} cold`);
    await adoptWatchedTaskSession(page);
    await page.waitForFunction(() => !document.querySelector("#inbox-list .inbox-offline"));
    await page.mouse.move(width - 1, height - 1);
    await assertVisibleWatchControls(page, `${label} online`);
    await page.locator("#inbox-rail").screenshot({ path: join(output, `inbox-watched-tasks-${label}.png`), animations: "disabled" });
    await disposeWatchedTaskInbox(page);
  }, { width, height, hasTouch });
}
