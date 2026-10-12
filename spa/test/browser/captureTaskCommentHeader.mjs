import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { LONG, modes, mountCommentHeaderPage, parkComment } from "./taskCommentHeaderFixture.mjs";

const output = resolve(process.argv[2] || "/tmp/task557-before");
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [deviceShim] });
try {
  for (const mode of modes) {
    await session.withPage(async ({ page, basePath }) => {
      await mountCommentHeaderPage(page, basePath, mode.theme);
      await parkComment(page, LONG, 500);
      await page.screenshot({ path: `${output}/${mode.label}-${mode.theme}.png` });
      await page.evaluate(() => window.__stickyPage.dispose());
    }, mode);
  }
} finally {
  await session.close();
}
console.log(`Captured comment headers in ${output}`);
