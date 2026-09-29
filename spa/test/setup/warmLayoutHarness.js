// The layout project's global setup: one throwaway check before any test runs.
//
// The first check a fresh machine runs pays for everything cold — Chromium's
// binary and Vite off the disk, esbuild's first start. On a CI runner that was
// 13–30 s on top of the check's own few seconds, all charged to whichever test
// happened to come first, which then timed out while its neighbours took two.
// Paid here, outside every test's budget, each test is timed for its own work.

import { withLayoutPage } from "../browser/layoutHarness.mjs";

export default async function warmLayoutHarness() {
  await withLayoutPage(async ({ page }) => page.evaluate(() => document.readyState));
}
