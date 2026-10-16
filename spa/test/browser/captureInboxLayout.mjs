// #629 evidence: both rail faces at desktop and 390px, from the same cached
// workspace/project/task fixture. No bridge or user's browser is involved.
import { mkdir } from "node:fs/promises";
import { resolve, join } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { disposeInboxLayout, mountInboxLayout, showInboxLayoutFace } from "./inboxLayoutSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/inbox-layout-629/after");
await mkdir(output, { recursive: true });
const session = await openLayoutSession();
try {
  for (const width of [1280, 390]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountInboxLayout(page, basePath);
      for (const face of ["inbox", "projects"]) {
        await showInboxLayoutFace(page, face);
        const path = join(output, `${face}-${width}.png`);
        await page.screenshot({ path });
        console.log(path);
      }
      await disposeInboxLayout(page);
    }, { width, height: 960, hasTouch: width === 390 });
  }
} finally {
  await session.close();
}
