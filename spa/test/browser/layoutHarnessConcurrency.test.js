import { expect, it } from "vitest";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

it("isolates concurrent layout servers' dependency caches", async () => {
  const cacheDirs = [];
  let ready = 0;
  let release;
  const bothReady = new Promise((resolve) => { release = resolve; });
  const loadApp = (plugins) => withLayoutPage(async ({ page, basePath, cacheDir }) => {
    cacheDirs.push(cacheDir);
    ready += 1;
    if (ready === 2) release();
    await bothReady;
    await loadBrowserModules(page, { app: "src/app.js" }, basePath);
    expect(await page.evaluate(() => Boolean(window.__layoutModules?.app))).toBe(true);
  }, { plugins });

  await Promise.all([loadApp([]), loadApp([deviceShim])]);
  expect(cacheDirs).toHaveLength(2);
  expect(new Set(cacheDirs).size).toBe(2);
}, 60_000);
