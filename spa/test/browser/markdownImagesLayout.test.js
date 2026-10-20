import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const IMAGE = '<svg xmlns="http://www.w3.org/2000/svg" width="700" height="160"><rect width="700" height="160" fill="#cfe9df"/><text x="24" y="88" font-size="24">Explicitly loaded image</text></svg>';
let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

for (const theme of ["light", "dark"]) {
  for (const [width, key] of [[960, "Enter"], [390, "Space"]]) {
    it(`waits for ${key} before requesting one external image at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        const requests = [];
        await page.route("https://images.fixture.test/**", (route) => {
          requests.push(route.request());
          return route.fulfill({ contentType: "image/svg+xml", body: IMAGE });
        });
        await loadBrowserModules(page, { markdown: "src/core/markdown.js", images: "src/core/markdownImages.js" }, basePath);
        const html = await page.evaluate(() => window.__layoutModules.markdown.markdownHtml('![Architecture diagram](https://images.fixture.test/diagram.svg "System overview")'));
        await mountLayout(page, `<main style="max-width:720px;margin:24px auto;padding:16px"><h1>Image privacy</h1><div class="markdown">${html}</div></main>`, { basePath });
        await page.evaluate((theme) => {
          document.documentElement.dataset.theme = theme;
          window.__disposeImages = window.__layoutModules.images.installMarkdownImages(document);
        }, theme);
        expect(await page.locator(".markdown img, .markdown [src]").count()).toBe(0);
        expect(requests).toHaveLength(0);
        const button = page.getByRole("button", { name: "Load image from images.fixture.test — Architecture diagram" });
        expect(await button.getAttribute("type")).toBe("button");
        await captureLayout(page, `images-${theme}-${width}-before.png`);
        await button.focus();
        expect(await button.evaluate((element) => getComputedStyle(element).outlineWidth)).toBe("2px");
        await page.keyboard.press(key);
        await page.locator(".markdown img").waitFor();
        await page.locator(".markdown img").evaluate(async (image) => { image.loading = "eager"; await image.decode(); });
        expect(requests).toHaveLength(1);
        expect(requests[0].headers().referer).toBeUndefined();
        expect(await page.locator(".md-image-load").count()).toBe(0);
        const fits = await page.evaluate(() => {
          const host = document.querySelector(".markdown");
          return document.querySelector("img").getBoundingClientRect().width <= host.getBoundingClientRect().width
            && document.documentElement.scrollWidth <= innerWidth;
        });
        expect(fits).toBe(true);
        await captureLayout(page, `images-${theme}-${width}-after.png`);
        await page.evaluate(() => window.__disposeImages());
      }, { width, height: 600 });
    });
  }
}

it("requires activation before requesting a same-origin development HTTP image", async () => {
  await session.withPage(async ({ page, basePath }) => {
    const origin = new URL(page.url()).origin;
    let requests = 0;
    await page.route(`${origin}/local-image.svg`, (route) => {
      requests += 1;
      return route.fulfill({ contentType: "image/svg+xml", body: IMAGE });
    });
    await loadBrowserModules(page, { markdown: "src/core/markdown.js", images: "src/core/markdownImages.js" }, basePath);
    const html = await page.evaluate((src) => window.__layoutModules.markdown.markdownHtml(`![Local](${src})`), `${origin}/local-image.svg`);
    await mountLayout(page, `<div class="markdown">${html}</div>`, { basePath });
    await page.evaluate(() => { window.__disposeImages = window.__layoutModules.images.installMarkdownImages(document); });
    expect(requests).toBe(0);
    expect(await page.locator("img").count()).toBe(0);
    await page.locator(".md-image-load").click();
    await page.locator("img").evaluate(async (image) => { image.loading = "eager"; await image.decode(); });
    expect(requests).toBe(1);
    await page.evaluate(() => window.__disposeImages());
  });
});

for (const hostClass of ["thread-event-detail", "cc-body"]) {
  it(`loads and sizes an image activated by keyboard in the ${hostClass} host`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      let requests = 0;
      await page.route("https://images.fixture.test/**", (route) => {
        requests += 1;
        return route.fulfill({ contentType: "image/svg+xml", body: IMAGE });
      });
      await loadBrowserModules(page, { markdown: "src/core/markdown.js", images: "src/core/markdownImages.js" }, basePath);
      const html = await page.evaluate(() => window.__layoutModules.markdown.markdownHtml("![Example](https://images.fixture.test/example.svg)", { mode: "inline" }));
      await mountLayout(page, `<main style="padding:16px"><div class="${hostClass}">${html}</div></main>`, { basePath });
      await page.evaluate(() => { window.__disposeImages = window.__layoutModules.images.installMarkdownImages(document); });
      expect(requests).toBe(0);
      const button = page.getByRole("button", { name: "Load image from images.fixture.test — Example" });
      expect(await button.evaluate((element) => element.getBoundingClientRect().height)).toBeGreaterThanOrEqual(44);
      await button.press("Enter");
      expect(await page.locator(`.${hostClass} img`).count()).toBe(1);
      await page.locator("img").evaluate(async (image) => { image.loading = "eager"; await image.decode(); });
      expect(requests).toBe(1);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.evaluate(() => window.__disposeImages());
    }, { width: 390, height: 600 });
  });
}

for (const activation of ["click", "Enter", "Space"]) {
  it(`separates outer-link navigation from linked-image ${activation} activation`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      let imageRequests = 0;
      let navigations = 0;
      await page.route("https://images.fixture.test/**", (route) => {
        imageRequests += 1;
        return route.fulfill({ contentType: "image/svg+xml", body: IMAGE });
      });
      await page.context().route("https://target.fixture.test/**", (route) => {
        navigations += 1;
        return route.fulfill({ contentType: "text/html", body: "<title>Linked target</title>Target" });
      });
      await loadBrowserModules(page, { markdown: "src/core/markdown.js", images: "src/core/markdownImages.js" }, basePath);
      const html = await page.evaluate(() => window.__layoutModules.markdown.markdownHtml("[![Linked badge](https://images.fixture.test/badge.svg)](https://target.fixture.test/docs)"));
      await mountLayout(page, `<div class="markdown">${html}</div>`, { basePath });
      await page.evaluate(() => { window.__disposeImages = window.__layoutModules.images.installMarkdownImages(document); });
      expect(imageRequests).toBe(0);
      expect(await page.locator("img, [src]").count()).toBe(0);
      expect(await page.locator("a.md-link > button.md-image-load").count()).toBe(1);

      const opened = page.context().waitForEvent("page");
      if (activation === "click") await page.locator("a.md-link").evaluate((link) => link.click());
      else {
        await page.locator("a.md-link").focus();
        await page.keyboard.press("Enter");
      }
      const target = await opened;
      await target.waitForLoadState();
      expect(navigations).toBe(1);
      expect(imageRequests).toBe(0);
      expect(await page.locator("img").count()).toBe(0);
      await target.close();

      const button = page.getByRole("button", { name: "Load image from images.fixture.test — Linked badge" });
      if (activation === "click") await button.click();
      else await button.press(activation);
      expect(await page.locator("a.md-link > img").count()).toBe(1);
      await page.locator("img").evaluate(async (image) => { image.loading = "eager"; await image.decode(); });
      expect(imageRequests).toBe(1);
      expect(navigations).toBe(1);
      expect(page.context().pages()).toHaveLength(1);
      expect(await page.locator("a.md-link").getAttribute("href")).toBe("https://target.fixture.test/docs");
      await page.evaluate(() => window.__disposeImages());
    }, { width: 390, height: 600 });
  });
}
