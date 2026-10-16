import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { LAYOUT_DEVICE, disposeInboxLayout, mountInboxLayout, showInboxLayoutFace } from "./inboxLayoutSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

it.each(["inbox", "projects"])("lets Tab reach a project agent and Enter open the click destination in %s", async (face) => {
  const routes = [];
  for (const activate of ["keyboard", "click"]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountInboxLayout(page, basePath);
      await showInboxLayoutFace(page, face);
      const row = page.locator(`[data-key="project-agent:${LAYOUT_DEVICE}/live-project"]`);
      if (activate === "keyboard") {
        await row.evaluate((element) => {
          const before = document.createElement("button");
          before.textContent = "Before project row";
          element.before(before);
          before.focus();
        });
        await page.keyboard.press("Tab");
        expect(await row.evaluate((element) => document.activeElement === element)).toBe(true);
        expect(await row.getAttribute("role")).toBe("link");
        await page.keyboard.press("Enter");
      } else await row.click();
      await page.waitForFunction(() => window.__layoutModules.app.App.route.name === "project");
      routes.push(await page.evaluate(() => window.__layoutModules.app.App.route));
      expect(routes.at(-1)).toEqual({ name: "project", deviceId: LAYOUT_DEVICE, projectId: "live-project" });
      await disposeInboxLayout(page);
    }, { width: 1280, height: 960 });
  }
  expect(routes[0]).toEqual(routes[1]);
}, 30_000);
