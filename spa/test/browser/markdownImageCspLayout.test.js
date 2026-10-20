import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { build } from "vite";
import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";

const config = readFileSync(resolve("../skriftapp/app.yaml"), "utf8");
const productionCsp = JSON.parse(config.match(/^  content_security_policy: (".*")$/m)[1]);
const documentHtml = readFileSync(resolve("test/browser/fixtures/markdownImageCsp.html"), "utf8");
const image = '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="24"><rect width="32" height="24" fill="green"/></svg>';
let session;
let fixture;

beforeAll(async () => {
  const bundle = await build({
    configFile: false, logLevel: "silent",
    build: { write: false, minify: false, lib: { entry: resolve("test/browser/fixtures/markdownImageCsp.js"), formats: ["es"] } },
  });
  const outputs = Array.isArray(bundle) ? bundle : [bundle];
  fixture = outputs.flatMap((output) => output.output).find((output) => output.type === "chunk" && output.isEntry).code;
  session = await openLayoutSession();
});
afterAll(async () => { await session?.close(); });

it("loads an explicitly activated HTTPS image under the exact production CSP header", async () => {
  await session.withPage(async ({ page }) => {
    const origin = new URL(page.url()).origin;
    let requests = 0;
    await page.route(`${origin}/csp-image-document`, (route) => route.fulfill({
      contentType: "text/html", headers: { "Content-Security-Policy": productionCsp }, body: documentHtml,
    }));
    await page.route(`${origin}/csp-image-fixture.js`, (route) => route.fulfill({ contentType: "text/javascript", body: fixture }));
    await page.route("https://images.fixture.test/diagram.svg", (route) => {
      requests += 1;
      return route.fulfill({ contentType: "image/svg+xml", body: image });
    });
    const response = await page.goto(`${origin}/csp-image-document`);
    expect(response.headers()["content-security-policy"]).toBe(productionCsp);
    await page.waitForFunction(() => window.__cspFixtureReady === true);
    expect(requests).toBe(0);
    expect(await page.locator("img, [src]:not(script)").count()).toBe(0);
    expect(await page.evaluate(() => window.__cspViolations)).toEqual([]);
    await page.getByRole("button", { name: "Load image from images.fixture.test — Production CSP diagram" }).click();
    const loaded = await page.locator("img").evaluate(async (image) => {
      image.loading = "eager";
      return image.decode().then(() => ({ decoded: true, width: image.naturalWidth }), () => ({ decoded: false, width: image.naturalWidth }));
    });
    expect(loaded.decoded).toBe(true);
    expect(loaded.width).toBeGreaterThan(0);
    expect(requests).toBe(1);
    expect(await page.evaluate(() => window.__cspViolations)).toEqual([]);
    await page.evaluate(() => window.__disposeImages());
  });
});
