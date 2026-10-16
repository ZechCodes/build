// Headless review evidence only: colored circles simulate native macOS buttons.
// Run: node test/browser/captureWindowControls.mjs /tmp/635-captures [before]
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { mountWindowControls } from "./windowControlsSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/635-captures");
const before = process.argv[3] === "before";
await mkdir(output, { recursive: true });
const session = await openLayoutSession({ plugins: [navigationAppShim] });
try {
  for (const pinned of [true, false]) await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountWindowControls(page, basePath, { install: !before, pinned, lights: true });
    await page.mouse.move(1279, 799);
    await page.screenshot({ path: `${output}/${before ? "before" : "after"}-inbox-${pinned ? "pinned" : "unpinned"}.png` });
  }, { width: 1280, height: 800 });
} finally { await session.close(); }
console.log(output);
