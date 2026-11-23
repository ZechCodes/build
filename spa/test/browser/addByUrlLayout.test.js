import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

it("keeps verified HTTPS identity and the explicit choice readable on a phone", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div id="scrim" class="scrim"><div id="sheet" class="sheet"></div></div>', { basePath });
    await loadBrowserModules(page, { url: "src/sheets/addByUrl.js" }, basePath);
    await page.evaluate(() => {
      window.__layoutModules.url.openAddByUrl(() => {}, { services: {
        begin: async () => ({ identity: { deviceId: "26b96687-aeeb-42eb-a8d9-43e00ab9e891", name: "Development laptop",
          origin: `https://${"bridge".repeat(10)}.${"private".repeat(8)}.example:9443`, fingerprint: "AB:CD:EF:01:02:03:04:05" } }),
        discard() {}, connect: async () => {}, navigate() {},
      } });
    });
    await page.locator("#bridge-url").fill("https://bridge.example:9443");
    await page.locator("#urlverify").click();
    await page.locator("#urlconnect").waitFor({ state: "visible" });
    const dimensions = await page.locator(".settings-sheet-body").evaluate((body) => ({ width: body.clientWidth, contentWidth: body.scrollWidth }));
    await captureLayout(page, "add-by-url-mobile.png");
    expect(dimensions.contentWidth).toBeLessThanOrEqual(dimensions.width + 1);
    expect(await page.locator("#urlconnect").textContent()).toBe("Use HTTPS");
    expect(await page.locator("#urlidentity").textContent()).toContain("Terminals also use HTTPS when supported. Older setups may use WebRTC.");
  }, { width: 390, height: 800 });
});

it("describes both terminal carriers in the device transport settings", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<section id="transport"></section>', { basePath });
    await loadBrowserModules(page, { transport: "src/core/httpTransportControls.js" }, basePath);
    await page.evaluate(() => {
      window.__layoutModules.transport.mountHttpTransportControls(document.querySelector("#transport"), "fixture-device");
    });
    expect(await page.locator("#transport").textContent()).toContain(
      "Terminals also use HTTPS when supported. Older setups may use WebRTC.",
    );
    expect(await page.getByLabel("Bridge HTTPS address").getAttribute("type")).toBe("url");
    expect(await page.getByRole("button", { name: "Save and use HTTPS" }).isVisible()).toBe(true);
  });
});
