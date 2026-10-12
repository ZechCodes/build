import { mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";
import { loadBrowserModules } from "./layoutHarness.mjs";

export async function mountNavigationPicker(page, basePath, options = {}) {
  await mountNavigationBreadcrumbs(page, basePath, options);
  await page.evaluate(() => { window.__navigationBaseModules = window.__layoutModules; });
  await loadBrowserModules(page, { ui: "src/core/localUiStore.js", visits: "src/core/navigationVisits.js" }, basePath);
  await page.evaluate(() => { Object.assign(window.__layoutModules, window.__navigationBaseModules); });
  await page.evaluate(async () => {
    const { app, cache, ui, visits } = window.__layoutModules;
    const deviceId = "away-device";
    const projects = [{ id: "build", name: "Build", deviceId, projectKey: `${deviceId}/build` }, { id: "docs", name: "Docs", deviceId, projectKey: `${deviceId}/docs` }];
    const workspace = (id, project_id, name) => ({ id, project_id, name, deviceId, projectKey: `${deviceId}/${project_id}`, workspaceKey: `${deviceId}/${id}`, status: "ready", watched: false, directories: [{ id: "docs-source", is_git: true }] });
    const workspaces = [workspace("current", "build", "navigation-plan"), workspace("release", "docs", "Café 文 release-notes"),
      ...Array.from({ length: 12 }, (_, index) => workspace(`extra-${index}`, "docs", `Release ${String(index).padStart(2, "0")}`))];
    app.App.devices.push({ id: deviceId, name: "Laptop" });
    app.App.deviceFilter = "nav-device";
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { projects, workspaces, items: [], runs: [] });
    await ui.writeUiRecord(visits.navigationVisitAddress, [
      { kind: "workspace", key: "nav-device/current", at: Date.now() - 1 },
      { kind: "workspace", key: "away-device/release", at: Date.now() - 2 },
    ]);
    window.__navigationPickerFixture = { deviceId, projects, workspaces };
  });
  await page.waitForFunction(() => document.querySelector("[data-project-return]")?.textContent.includes("Workshop"));
}

export async function tickNavigationPicker(page, { remove = false } = {}) {
  await page.evaluate(async ({ remove }) => {
    const { cache } = window.__layoutModules;
    const fixture = window.__navigationPickerFixture;
    const { deviceId, projects } = fixture;
    const workspaces = fixture.workspaces.filter((row) => !remove || row.id !== "release").map((row) => row.id === "release" ? { ...row, name: "A renamed release", status: "failed" } : row);
    workspaces.push({ ...workspaces.find((row) => row.project_id === "docs"), id: "new", workspaceKey: `${deviceId}/new`, name: "A newcomer" });
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { projects, workspaces, items: [], runs: [] });
  }, { remove });
}
