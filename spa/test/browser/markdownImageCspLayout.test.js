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

// CDP interception includes redirected requests, unlike page.route handlers
// which only handle a redirect chain's first request. Fail every unknown URL
// closed so this production-origin regression never uses the real network.
async function interceptRedirectFixture(page) {
  const origin = "https://build.fixture.test";
  const target = "https://github.com/build-fixture/releases/download/v1/diagram.svg";
  const requests = { sameOrigin: 0, external: 0 };
  const unexpected = [];
  await page.context().unrouteAll();
  const cdp = await page.context().newCDPSession(page);
  const answer = (url) => {
    if (url.startsWith(`${origin}/csp-image-document?`)) return { type: "text/html", body: documentHtml, headers: [{ name: "Content-Security-Policy", value: productionCsp }] };
    if (url === `${origin}/csp-image-fixture.js`) return { type: "text/javascript", body: fixture };
    if (url === `${origin}/app/downloads/SHA256SUMS`) {
      requests.sameOrigin += 1;
      return { status: 302, headers: [{ name: "Location", value: target }] };
    }
    if (url === target) {
      requests.external += 1;
      return { type: "image/svg+xml", body: image };
    }
    if (url === `${origin}/favicon.ico`) return { status: 204 };
    return null;
  };
  const failures = [];
  const paused = async ({ requestId, request }) => {
    const response = answer(request.url);
    if (!response) {
      unexpected.push(request.url);
      await cdp.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
      return;
    }
    await cdp.send("Fetch.fulfillRequest", {
      requestId, responseCode: response.status || 200,
      responseHeaders: [{ name: "Content-Type", value: response.type || "text/plain" }, ...(response.headers || [])],
      body: Buffer.from(response.body || "").toString("base64"),
    });
  };
  cdp.on("Fetch.requestPaused", (event) => { paused(event).catch((error) => failures.push(String(error))); });
  await cdp.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Request" }] });
  return { origin, requests, unexpected, failures, close: () => cdp.detach() };
}

for (const linked of [false, true]) {
  it(`makes no same-origin redirect or external request before activation under production CSP (linked: ${linked})`, async () => {
    await session.withPage(async ({ page }) => {
      const routes = await interceptRedirectFixture(page);
      try {
        const response = await page.goto(`${routes.origin}/csp-image-document?redirect${linked ? "&linked" : ""}`);
        expect(response.headers()["content-security-policy"]).toBe(productionCsp);
        await page.waitForFunction(() => window.__cspFixtureReady === true);
        // Attempt any premature fetch before the assertion, without making a
        // decoding error conceal the initial-request regression in red runs.
        if (await page.locator("img").count()) await page.locator("img").evaluate(async (image) => { image.loading = "eager"; await image.decode().catch(() => {}); });
        expect(routes.requests).toEqual({ sameOrigin: 0, external: 0 });
        expect(await page.locator("img, [src]:not(script)").count()).toBe(0);
        expect(await page.locator("a.md-link").count()).toBe(linked ? 1 : 0);
        await page.getByRole("button", { name: "Load image from build.fixture.test — Production CSP diagram" }).click();
        const width = await page.locator("img").evaluate(async (image) => { image.loading = "eager"; await image.decode(); return image.naturalWidth; });
        expect(width).toBeGreaterThan(0);
        expect(routes.requests).toEqual({ sameOrigin: 1, external: 1 });
        expect(routes.unexpected).toEqual([]);
        expect(routes.failures).toEqual([]);
        expect(await page.evaluate(() => window.__cspViolations)).toEqual([]);
        await page.evaluate(() => window.__disposeImages());
      } finally {
        await routes.close();
      }
    });
  });
}
