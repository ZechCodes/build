import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const paths = ["a.txt", "b.txt", "c.txt", "d.txt"];

async function mountTruncatedReview(page, basePath, theme) {
  await mountLayout(page, '<main class="surface"><h1>Changes</h1><div id="review" class="cdetail-host"></div></main>', {
    basePath,
    styles: 'main { padding: 24px; height: 100vh; box-sizing: border-box; } h1 { font-size: 18px; margin-bottom: 16px; } #review { height: calc(100vh - 90px); overflow: auto; }',
  });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await loadBrowserModules(page, { review: "src/core/changesReview.js", cache: "src/core/localCache.js" }, basePath);
  await page.evaluate(async (paths) => {
    const { review, cache } = window.__layoutModules;
    const deviceId = "review-truncated-559";
    const cacheScope = { deviceId, address: (parts) => ({ ...parts, deviceId }) };
    const rowOf = (path) => ({ path, status: "Modified", additions: 1, deletions: 1, content_key: `body-${path}` });
    const patchFor = (path) => [
      `diff --git a/${path} b/${path}`,
      "index 0000000..1111111 100644",
      `--- a/${path}`, `+++ b/${path}`, "@@ -1 +1 @@", "-old", `+${path} body`, "",
    ].join("\n");
    const address = cacheScope.address({ entityId: "run-559", kind: "diff" });
    await cache.writeCached(address, { files: [], patch: "", stale: true, diff_key: "old", commentable: false });
    window.__fileRequests = [];
    window.__reviewPlug = review.createReviewPlug({
      cacheScope, entity: "run-559",
      fetchDiff: async () => ({
        files: paths.map(rowOf), patch: paths.slice(0, 2).map(patchFor).join(""),
        truncated: true, diff_key: "truncated-559", commentable: false,
      }),
      fetchFiles: async (requested) => {
        window.__fileRequests.push([...requested]);
        return { files: requested.map(rowOf), patch: requested.map(patchFor).join("") };
      },
    });
    window.__reviewPlug.mount(document.getElementById("review"));
  }, paths);
}

for (const theme of ["light", "dark"]) {
  it(`fetches omitted file bodies after a truncated aggregate in ${theme} theme (#559)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountTruncatedReview(page, basePath, theme);
      try {
        await expect.poll(() => page.locator("#review .file").evaluateAll((files) => files.map((file) => ({
          path: file.querySelector(".fpath")?.textContent,
          hunks: file.querySelectorAll("tr.hunk").length,
        }))), { timeout: 5_000 }).toEqual(paths.map((path) => ({ path, hunks: 1 })));
        expect(await page.locator("#review").textContent()).not.toContain("loading…");
        const requested = await page.evaluate(() => window.__fileRequests.flat());
        expect(requested).toEqual(expect.arrayContaining(paths.slice(2)));
      } finally {
        await captureLayout(page, `changes-review-truncated-${theme}.png`);
        await page.evaluate(() => window.__reviewPlug.unmount());
      }
    }, { width: 1280, height: 800 });
  }, 60_000);
}
