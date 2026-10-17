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

it.each(["inbox", "projects"])("keeps project navigation focused across a live reorder in %s", async (face) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountInboxLayout(page, basePath);
    await showInboxLayoutFace(page, face);
    const row = page.locator(`[data-key="project-agent:${LAYOUT_DEVICE}/live-project"]`);
    const routeBefore = await page.evaluate(() => window.__layoutModules.app.App.route);
    await row.focus();
    await page.evaluate(async (deviceId) => {
      window.__focusedProjectRow = document.activeElement;
      const address = { deviceId, entityId: "", kind: "workspaces" };
      const { value: rows } = await window.__layoutModules.cache.readCached(address);
      await window.__layoutModules.cache.writeCached(address, rows.map((workspace) =>
        workspace.id === "quiet-workspace" ? { ...workspace,
          session_started_ms: Date.now() - 3600_000, last_activity_ms: Date.now() } : workspace));
    }, LAYOUT_DEVICE);
    await page.waitForFunction(({ face, deviceId }) => face === "projects"
      ? document.querySelector(".inbox-project")?.dataset.project === `${deviceId}/quiet-project`
      : document.querySelector("#inbox-list > .inbox-entry")?.dataset.key === `workspace:${deviceId}/quiet-workspace`,
    { face, deviceId: LAYOUT_DEVICE });
    expect(await row.evaluate((element) => element === window.__focusedProjectRow)).toBe(true);
    expect(await row.evaluate((element) => document.activeElement === element)).toBe(true);
    expect(await page.evaluate(() => window.__layoutModules.app.App.route)).toEqual(routeBefore);
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__layoutModules.app.App.route.name === "project");
    expect(await page.evaluate(() => window.__layoutModules.app.App.route))
      .toEqual({ name: "project", deviceId: LAYOUT_DEVICE, projectId: "live-project" });
    await disposeInboxLayout(page);
  }, { width: 1280, height: 960 });
}, 30_000);
