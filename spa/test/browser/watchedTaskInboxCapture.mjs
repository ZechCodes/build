// #475 review screenshots, using the same cold cached production inbox as its browser gate.
// From spa/: nice -n 10 node test/browser/watchedTaskInboxCapture.mjs
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { withLayoutPage } from "./layoutHarness.mjs";
import { QUIET_TASK, disposeWatchedTaskInbox, mountWatchedTaskInbox } from "./watchedTaskInboxSeed.mjs";

const output = process.env.CAPTURE_DIR || fileURLToPath(new URL("../../../design/inbox-watched-tasks/", import.meta.url));
await mkdir(output, { recursive: true });
for (const { label, width, height, hasTouch } of [
  { label: "desktop", width: 1280, height: 440, hasTouch: false },
  { label: "phone", width: 390, height: 760, hasTouch: true },
]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWatchedTaskInbox(page, basePath);
    await page.locator(`[data-key="tracker_task:${QUIET_TASK}"] .inbox-watch`).waitFor({ state: "visible" });
    await page.mouse.move(width - 1, height - 1);
    await page.locator("#inbox-rail").screenshot({ path: join(output, `inbox-watched-tasks-${label}.png`), animations: "disabled" });
    await disposeWatchedTaskInbox(page);
  }, { width, height, hasTouch });
}
