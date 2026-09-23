import { constants } from "node:fs";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const spaRoot = fileURLToPath(new URL("../../", import.meta.url));
const chromiumNames = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];

async function chromiumExecutable() {
  const configured = process.env.CHROMIUM_PATH;
  if (configured) {
    await access(configured, constants.X_OK);
    return configured;
  }
  for (const directory of (process.env.PATH || "").split(delimiter)) {
    for (const name of chromiumNames) {
      const candidate = join(directory, name);
      try {
        await access(candidate, constants.X_OK);
        return candidate;
      } catch { /* Try the next browser location. */ }
    }
  }
  throw new Error("Browser layout tests require Chromium or Chrome on PATH (or CHROMIUM_PATH)");
}

/** Run a real Chromium layout check against the SPA's production renderers and
 * styles. Each call owns its Vite server and browser, so failures also clean up. */
export async function withLayoutPage(check, { width = 1180, height = 840, plugins = [] } = {}) {
  const chromiumPath = await chromiumExecutable();
  // Browser suites start Vite servers in parallel. Each optimizer must own its
  // cache or another server can replace dependency files mid-import.
  const cacheDir = await mkdtemp(join(tmpdir(), "build-layout-vite-"));
  let server;
  let browser;
  try {
    server = await createServer({
      root: spaRoot, cacheDir, logLevel: "silent", plugins,
      server: { host: "127.0.0.1", port: 0 },
    });
    await server.listen();
    const port = server.httpServer.address().port;
    const basePath = server.config.base;
    browser = await chromium.launch({ executablePath: chromiumPath, headless: true, args: ["--no-sandbox"] });
    const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
    // A CSS URL gives the page Vite's origin without booting the SPA. That lets
    // a test mount just the production renderer it needs into a stable shell.
    await page.goto(`http://127.0.0.1:${port}${basePath}src/styles.css`);
    return await check({ page, basePath, cacheDir: server.config.cacheDir });
  } finally {
    try {
      await browser?.close();
    } finally {
      try {
        await server?.close();
      } finally {
        await rm(cacheDir, { recursive: true, force: true });
      }
    }
  }
}

export async function mountLayout(page, markup, { styles = "", basePath = "/app/static/" } = {}) {
  await page.setContent(`<html><head><link rel="stylesheet" href="${basePath}src/styles.css">
    <link rel="stylesheet" href="${basePath}src/styles/shell.css">
    <style>${styles}</style></head><body>${markup}</body></html>`, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
}

/** Load production ES modules in the browser itself. Vitest rewrites dynamic
 * import expressions in test callbacks, so page.evaluate must read these from
 * window rather than importing within its serialized callback. */
export async function loadBrowserModules(page, modules, basePath = "/app/static/") {
  const names = Object.keys(modules);
  const paths = Object.values(modules).map((path) => `${basePath}${path}`);
  const failures = [];
  const requestFailed = (request) => failures.push(`${request.failure()?.errorText || "request failed"} ${request.url()}`);
  const badResponse = (response) => {
    if (response.status() < 400) return;
    failures.push(response.text().then((body) =>
      `${response.status()} ${response.url()} ${body.slice(0, 500)}`,
    ).catch(() => `${response.status()} ${response.url()}`));
  };
  page.on("requestfailed", requestFailed);
  page.on("response", badResponse);
  try {
    await page.addScriptTag({ type: "module", content: `
      Promise.all(${JSON.stringify(paths)}.map((path) => import(path)))
        .then((loaded) => { window.__layoutModules = Object.fromEntries(${JSON.stringify(names)}.map((name, index) => [name, loaded[index]])); })
        .catch((error) => { window.__layoutModuleError = String(error); });
    ` });
    await page.waitForFunction(() => window.__layoutModules || window.__layoutModuleError);
    const error = await page.evaluate(() => window.__layoutModuleError);
    if (error) throw new Error(`${error}; network: ${(await Promise.all(failures)).join(" | ") || "no failed requests"}`);
  } finally {
    page.off("requestfailed", requestFailed);
    page.off("response", badResponse);
  }
}
