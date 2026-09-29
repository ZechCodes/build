// #228: a device's project row ends in its Settings… button, pinned to the
// row's right edge whatever the path, facts and source count before it take.
// Checked in Chromium against the production renderer and styles, with the
// panel standing on its own. (The settings pages wrap the row and start the
// button on its own line; that is their rule, not this one.)
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const PROJECTS = [
  { project_id: "proj-1", name: "Do", path: "/home/zech/Projects/Do", base_branch: "main", is_git: true,
    isolation_effective: "rift", sources: [{ id: "source-1" }, { id: "source-2" }] },
  { project_id: "proj-2", name: "Notes", path: "/n", base_branch: "main", is_git: false,
    isolation_effective: "worktree", sources: [{ id: "source-1" }] },
];

for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 900 },
  { label: "mobile", width: 390, height: 800 },
]) {
  it(`pins each project's Settings… button to the row's right edge on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<div id="host" style="max-width:1100px;margin:0 auto;padding:0 16px"></div>', { basePath });
      await loadBrowserModules(page, { panel: "src/views/deviceProjects.js" }, basePath);
      await page.evaluate(async (projects) => {
        const { panel } = window.__layoutModules;
        const host = document.getElementById("host");
        host.innerHTML = panel.deviceProjectsPanelHtml();
        await panel.mountDeviceProjects(host, {
          deviceId: "laptop",
          deviceName: "Laptop",
          callRpc: async () => ({ projects }),
        });
      }, PROJECTS);
      await page.waitForFunction(() => document.querySelectorAll(".projrow .projsettings").length === 2);

      const rows = await page.evaluate(() => [...document.querySelectorAll(".projrow")].map((row) => {
        const box = row.getBoundingClientRect();
        const button = row.querySelector(".projsettings").getBoundingClientRect();
        const count = [...row.querySelectorAll("span")].find((span) => /sources?$/.test(span.textContent));
        return { rowRight: box.right, buttonRight: button.right, buttonHeight: button.height,
          countLines: Math.round(count.getBoundingClientRect().height / parseFloat(getComputedStyle(count).lineHeight)) };
      }));

      await captureLayout(page, `device-project-rows-${label}.png`);
      expect(rows).toHaveLength(2);
      for (const row of rows) {
        expect(Math.abs(row.rowRight - row.buttonRight)).toBeLessThanOrEqual(1);
        expect(row.buttonHeight).toBeLessThan(40);
        expect(row.countLines).toBe(1);
      }
    }, { width, height });
  });
}
