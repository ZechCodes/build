import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountBlockedInbox, mountBlockedTasks } from "./taskBlockedSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/task563-evidence/screenshots");
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [deviceShim] });
try {
  for (const width of [390, 1280]) {
    for (const theme of ["light", "dark"]) {
      const capture = async (page, surface) => {
        const path = resolve(output, `blocked-${surface}-${width}-${theme}.png`);
        await page.screenshot({ path, fullPage: true });
        console.log(path);
      };
      await session.withPage(async ({ page, basePath }) => {
        await mountBlockedTasks(page, basePath, { theme });
        await page.locator('[data-dashboard-tab="active"]').click();
        await page.locator('[data-active-group="blocked"]').waitFor();
        await capture(page, "active-rail");
        await page.locator('[data-dashboard-tab="backlog"]').click();
        await page.locator('.task-dashboard [data-backlog-group="blocked"]').waitFor();
        await capture(page, "backlog");
        await page.locator('[data-task-view="board"]').click();
        await page.locator('[data-column="backlog"] [data-backlog-group="blocked"]').waitFor();
        await capture(page, "board");
      }, { width, height: 1000 });
      await session.withPage(async ({ page, basePath }) => {
        await mountBlockedInbox(page, basePath, { theme, question: true });
        await capture(page, "inbox");
      }, { width, height: 1000 });
    }
  }
} finally {
  await session.close();
}
