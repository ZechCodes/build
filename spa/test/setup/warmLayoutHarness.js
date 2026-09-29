// The layout project's global setup: one throwaway check before any test runs.
//
// The first check a fresh machine runs pays for everything cold — Chromium's
// binary and Vite off the disk, esbuild's first start. On a CI runner that was
// 13–30 s on top of the check's own few seconds, all charged to whichever test
// happened to come first, which then timed out while its neighbours took two.
// Paid here, outside every test's budget, each test is timed for its own work.

import { withLayoutPage } from "../browser/layoutHarness.mjs";

// Far past the worst cold start seen, so only a hang reaches it — and a hang
// then fails the run here instead of holding CI until the job's own limit.
const WARM_UP_BUDGET_MS = 120_000;

export default async function warmLayoutHarness() {
  let timer;
  const hung = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`the layout harness did not start in ${WARM_UP_BUDGET_MS} ms`)), WARM_UP_BUDGET_MS);
  });
  try {
    await Promise.race([withLayoutPage(async ({ page }) => page.evaluate(() => document.readyState)), hung]);
  } finally {
    clearTimeout(timer);
  }
}
