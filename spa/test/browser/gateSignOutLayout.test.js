import { readFileSync } from "node:fs";
import { afterAll, beforeAll, expect, it } from "vitest";
import { downloadsPayload } from "../downloadsFixture.js";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const body = readFileSync(new URL("../../index.html", import.meta.url), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1];
let session;
beforeAll(async () => { session = await openLayoutSession(); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const width of [390, 1280]) {
  for (const theme of ["light", "dark"]) {
    it.each(["welcome", "settings"])(`keeps %s Sign out visible without scrolling at ${width}px in ${theme}`, async (surface) => {
      await session.withPage(async ({ page, basePath }) => {
        const devices = surface === "welcome" ? [] : [
          { id: "workstation", name: "Workstation", fingerprint: "AABBCCDD", status: "offline" },
        ];
        await page.route("**/api/devices", (route) => route.fulfill({ json: { devices } }));
        await page.route("**/app/downloads", (route) => route.fulfill({ json: downloadsPayload() }));
        await mountLayout(page, body, { basePath });
        await loadBrowserModules(page, {
          gate: "src/views/gate.js", settings: "src/views/settingsModal.js",
          app: "src/app.js", cache: "src/core/localCache.js",
        }, basePath);
        await page.evaluate(async ({ theme, surface, devices }) => {
          document.documentElement.dataset.theme = theme;
          const { gate, settings, app, cache } = window.__layoutModules;
          if (surface === "welcome") await gate.boot();
          else {
            await cache.writeCached(cache.DEVICES_ADDRESS, devices);
            Object.assign(app.App, { devices, gated: false, route: { name: "account", page: "settings" } });
            settings.renderSettingsModal();
          }
          await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        }, { theme, surface, devices });

        const signOut = page.getByRole("link", { name: "Sign out", exact: true });
        await signOut.waitFor();
        expect(await signOut.count()).toBe(1);
        const bounds = await signOut.boundingBox();
        expect(bounds.y).toBeGreaterThanOrEqual(0);
        expect(bounds.y + bounds.height).toBeLessThan(700);
        expect(bounds.x).toBeGreaterThanOrEqual(0);
        expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
        expect(await signOut.getAttribute("class")).not.toContain("primary");
        await captureLayout(page, `${surface}-sign-out-${width}-${theme}.png`);

        // Follow the real link without contacting an auth server or pairing a device.
        await page.route("**/auth/logout", (route) => route.fulfill({
          contentType: "text/html", body: "<h1>Log out of Build?</h1>",
        }));
        await signOut.click();
        expect(new URL(page.url()).pathname).toBe("/auth/logout");
      }, { width, height: 700, hasTouch: width === 390 });
    });
  }
}
