// node test/browser/captureInteraction.mjs /tmp/build-397-review before|after
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { dragInteractionText, mountInteractionFixture } from "./interactionFixture.mjs";

const directory = process.argv[2] || "/tmp/build-397-review";
const phase = process.argv[3] || "after";
if (!["before", "after"].includes(phase)) throw new Error("Capture phase must be before or after");
await mkdir(directory, { recursive: true });
await withLayoutPage(async ({ page, basePath }) => {
  await mountInteractionFixture(page, basePath);
  for (const [name, selector] of [["header", ".tb-legacy-item .tb-name"], ["comment", ".task-comment-body p"]]) {
    const selected = await dragInteractionText(page, selector);
    const path = join(directory, `${phase}-${name}.png`);
    await page.screenshot({ path });
    console.log(JSON.stringify({ path, selected }));
  }
}, { width: 1280, height: 900 });
