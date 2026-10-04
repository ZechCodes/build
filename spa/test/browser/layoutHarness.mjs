import { constants } from "node:fs";
import { access, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const spaRoot = fileURLToPath(new URL("../../", import.meta.url));
// The fonts every check measures in, whatever the machine's defaults (fonts.conf).
const pinnedFonts = fileURLToPath(new URL("./fonts.conf", import.meta.url));
const chromiumNames = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];

function moduleRequestUrl(requestUrl, basePath) {
  const url = new URL(requestUrl);
  const path = url.pathname.startsWith(basePath) ? `/${url.pathname.slice(basePath.length)}` : url.pathname;
  const normalized = decodeURI(`${path}${url.search}`).replace("__x00__", "\0")
    .replace(/(\?|&)import=?(?=&|$)/, "$1").replace(/[?&]$/, "");
  return normalized.startsWith("/@id/") ? normalized.slice(5) : normalized;
}

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
export async function withLayoutPage(check, { width = 1180, height = 840, plugins = [], deviceScaleFactor = 1 } = {}) {
  const chromiumPath = await chromiumExecutable();
  // Browser suites start Vite servers in parallel. Each optimizer must own its
  // cache or another server can replace dependency files mid-import.
  const cacheDir = await mkdtemp(join(tmpdir(), "build-layout-vite-"));
  let server;
  let browser;
  try {
    server = await createServer({
      root: spaRoot, cacheDir, logLevel: "silent", plugins,
      optimizeDeps: {
        // Prebundle CJS grammars before the page imports a renderer. Discovery
        // during an import can invalidate dependency requests already in flight.
        noDiscovery: true,
        include: [
          "ghostty-web",
          ...["core", "markup", "clike", "css", "javascript", "jsx", "typescript",
            "tsx", "python", "rust", "json", "bash", "yaml", "toml", "markdown"]
            .map((grammar) => `prismjs/components/prism-${grammar}`),
        ],
        // Vite's ESM resolver shim handles this package's ./libsodium.mjs
        // import; the dependency optimizer bypasses that shim.
        exclude: ["libsodium-wrappers"],
      },
      server: { host: "127.0.0.1", port: 0 },
    });
    await server.listen();
    const port = server.httpServer.address().port;
    const basePath = server.config.base;
    browser = await chromium.launch({
      executablePath: chromiumPath, headless: true, args: ["--no-sandbox"],
      env: { ...process.env, FONTCONFIG_FILE: pinnedFonts },
    });
    const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor });
    // Serve real Vite-transformed modules through the driver, without an HTTP
    // proxy or retained APIResponse bodies. Host network notifications can
    // cancel Chromium's loopback module graph even when the server is healthy.
    await page.route(`http://127.0.0.1:${port}/**`, async (route) => {
      if (route.request().resourceType() !== "script") return route.continue();
      let result;
      try {
        result = await server.environments.client.transformRequest(moduleRequestUrl(route.request().url(), basePath));
      } catch (error) {
        return route.fulfill({ status: 500, contentType: "text/plain", body: String(error) });
      }
      return route.fulfill({ status: result ? 200 : 404, contentType: "text/javascript", body: result?.code || "Module not found" });
    });
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

async function replaceLayoutBody(page, markup, styles, basePath) {
  await page.evaluate(async ({ markup, styles, basePath }) => {
    // Retain imported modules' injected styles and mount-once document
    // listeners. Only this fixture's body and custom styles are replaced.
    const loaded = ["src/styles.css", "src/styles/shell.css"].map((path) => {
      const href = `${basePath}${path}`;
      if (document.head.querySelector(`link[href="${href}"]`)) return Promise.resolve();
      const link = document.createElement("link");
      link.rel = "stylesheet";
      link.href = href;
      const ready = new Promise((resolve, reject) => {
        link.onload = resolve;
        link.onerror = () => reject(new Error(`Failed to load layout stylesheet ${href}`));
      });
      document.head.appendChild(link);
      return ready;
    });
    document.querySelector("#layout-fixture-styles")?.remove();
    const fixtureStyles = document.createElement("style");
    fixtureStyles.id = "layout-fixture-styles";
    fixtureStyles.textContent = styles;
    document.head.appendChild(fixtureStyles);
    const body = document.createElement("body");
    body.innerHTML = markup;
    document.body.replaceWith(body);
    await Promise.all(loaded);
  }, { markup, styles, basePath });
}

export async function mountLayout(page, markup, { styles = "", basePath = "/app/static/", preserveDocument = false } = {}) {
  if (preserveDocument) await replaceLayoutBody(page, markup, styles, basePath);
  else {
    await page.setContent(`<html><head><link rel="stylesheet" href="${basePath}src/styles.css">
      <link rel="stylesheet" href="${basePath}src/styles/shell.css">
      <style>${styles}</style></head><body>${markup}</body></html>`, { waitUntil: "load" });
  }
  await page.evaluate(() => document.fonts.ready);
}

/** Optional review artifacts; normal gates do not rewrite tracked images. */
export async function captureLayout(page, name) {
  const directory = process.env.BUILD_LAYOUT_SCREENSHOTS;
  if (!directory) return;
  await mkdir(directory, { recursive: true });
  await page.screenshot({ path: join(directory, name) });
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
    // A preceding load must not satisfy readiness for this module set.
    await page.evaluate(() => { delete window.__layoutModules; delete window.__layoutModuleError; });
    await page.addScriptTag({ type: "module", content: `
      Promise.all(${JSON.stringify(paths)}.map((path) => import(path)))
        .then((loaded) => { window.__layoutModules = Object.fromEntries(${JSON.stringify(names)}.map((name, index) => [name, loaded[index]])); })
        .catch((error) => { window.__layoutModuleError = String(error); });
    ` });
    // A full renderer graph needs its own bound, independent of field/action
    // timeouts when several browser suites compile modules on the same host.
    await page.waitForFunction(() => window.__layoutModules || window.__layoutModuleError, null, { timeout: 15_000 });
    const error = await page.evaluate(() => window.__layoutModuleError);
    if (error) throw new Error(`${error}; network: ${(await Promise.all(failures)).join(" | ") || "no failed requests"}`);
  } finally {
    page.off("requestfailed", requestFailed);
    page.off("response", badResponse);
  }
}
