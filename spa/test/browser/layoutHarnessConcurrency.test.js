import { expect, it } from "vitest";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";

it.each(["modules", "error"])("waits for this import when a previous %s result is present", async (stale) => {
  const plugin = {
    name: "delayed-layout-module",
    resolveId(id) { if (id.endsWith("/test/slow-loader-fixture.js")) return id; },
    load(id) {
      if (id.endsWith("/test/slow-loader-fixture.js")) return "await new Promise(resolve => setTimeout(resolve, 250)); export const ready = true;";
    },
  };
  await withLayoutPage(async ({ page, basePath }) => {
    await page.evaluate((stale) => {
      window.__layoutModules = stale === "modules" ? { previous: {} } : undefined;
      window.__layoutModuleError = stale === "error" ? "Previous import failed" : undefined;
    }, stale);
    await loadBrowserModules(page, { replacement: "test/slow-loader-fixture.js" }, basePath);
    expect(await page.evaluate(() => window.__layoutModules.replacement.ready)).toBe(true);
    expect(await page.evaluate(() => Object.keys(window.__layoutModules))).toEqual(["replacement"]);
  }, { plugins: [plugin] });
}, 30_000);

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
