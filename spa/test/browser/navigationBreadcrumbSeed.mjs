import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
const html = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];

// Only app navigation is isolated. Canonical cache, feed, App state, toolbar
// controller/renderers and shell CSS run without a bridge or device session.
export const navigationAppShim = {
  name: "navigation-breadcrumb-app-shim", enforce: "pre",
  resolveId(source) { return /(?:^|\/)app\.js$/.test(source) ? "\0navigation-breadcrumb-app" : null; },
  load(id) {
    if (id !== "\0navigation-breadcrumb-app") return null;
    return `import { App } from '/src/appState.js'; export { App };
      export function go(route) { window.__navigationHarness.navigate(route); }
      export function render() {} export function rememberSelectedDevice() {}`;
  },
};
export async function mountNavigationBreadcrumbs(page, basePath, { theme = "light", longNames = false, kind = "workspace", duplicateNames = false } = {}) {
  if (await page.evaluate(() => !!window.__navigationHarness)) {
    await page.evaluate(async () => { await window.__layoutModules.toolbar.stopToolbar(); window.__layoutModules.feed.stopFeed(); });
  }
  await mountLayout(page, body, { basePath, preserveDocument: true });
  await page.evaluate(() => {
    if (!document.querySelector('meta[name="viewport"]')) document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    app: "src/appState.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", toolbar: "src/core/toolbar.js",
    router: "src/core/router.js", projectState: "src/core/projectRailState.js", lock: "src/core/workspaceLockSupport.js", tracker: "src/core/trackerCache.js", directoryRail: "src/core/directoryRail.js",
  }, basePath);
  await page.evaluate(async ({ theme, longNames, kind, duplicateNames }) => {
    const { app, cache, feed, toolbar, router, projectState, lock, tracker, directoryRail } = window.__layoutModules;
    const deviceId = "nav-device";
    document.documentElement.dataset.theme = theme;
    if (innerWidth < 900) document.body.classList.add("inbox-collapsed");
    const projectName = longNames ? "Navigation evidence with an unusually long project name for independent ellipsis!" : "Build";
    const workspaceName = longNames ? "An unusually long workspace name with enough text to stress the narrow toolbar row" : "navigation-plan";
    const projects = [{ id: "build", name: projectName, deviceId, projectKey: `${deviceId}/build`, sources: [{ id: "source", path: "/fixture/build" }] }, { id: "docs", name: "Docs", deviceId, projectKey: `${deviceId}/docs` }];
    const workspace = (id, name) => ({ id, workspace_id: id, project_id: "build", deviceId, projectKey: `${deviceId}/build`, workspaceKey: `${deviceId}/${id}`, name, status: "ready", locked: true, managed: true, directories: [{ id: "source", source_id: "source", name: "Build", is_git: true }] });
    const workspaces = [workspace("current", workspaceName), workspace("sibling", "Sibling workspace")];
    const route = { name: "workspace", deviceId, projectId: "build", workspaceId: "current", tab: "files", sourceId: "source", file: "README.md", line: 12 };
    const routes = { workspace: route, workspaceTask: { ...route, tab: "tasks", taskId: "nav-task", view: "board" }, projectTask: { name: "trackerTask", deviceId, projectId: "build", taskId: "nav-task" }, branch: { name: "branch", deviceId, projectId: "build", branch: "build/navigation-evidence", tab: "files", file: "README.md" }, project: { name: "project", deviceId, projectId: "build", tab: "files", sourceId: "source", file: "README.md", line: 12 } };
    app.App.devices = [{ id: deviceId, name: "Workshop" }];
    if (duplicateNames) app.App.devices.push({ id: "second-device", name: "Laptop" }); app.App.selectedDeviceId = deviceId; app.App.route = routes[kind];
    location.hash = router.hashFromRoute(app.App.route);
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { projects, workspaces, items: [], runs: [] });
    if (duplicateNames) {
      const other = [{ id: "build", name: projectName, deviceId: "second-device", projectKey: "second-device/build" }];
      await cache.writeCached({ deviceId: "second-device", entityId: "", kind: "projects" }, other);
      await cache.writeCached({ deviceId: "second-device", entityId: "", kind: "workspaces" }, []);
      await cache.writeCached({ deviceId: "second-device", entityId: "", kind: "feed" }, { projects: other, workspaces: [], items: [], runs: [] });
    }
    await lock.rememberWorkspaceLockSupport(deviceId, { workspaces: { setLocked: true } });
    await projectState.rememberProjectRailRoute(routes.project);
    await tracker.writeTaskRecord(deviceId, "build", "nav-task", { task: { id: "nav-task", number: 542, title: "Navigation: one-row breadcrumbs and focused sibling switcher", project_id: "build", assignee: { kind: "agent", workspace_id: "elsewhere" } }, timeline: [] });
    window.__navigationHarness = { navigations: [], focusInGesture: null, navigate(next) { this.navigations.push(next); app.App.route = next; location.hash = router.hashFromRoute(next); toolbar.toolbarRouteChanged(); } };
    if (!window.__navigationHashObserver) {
      window.addEventListener("hashchange", () => {
        const modules = window.__layoutModules;
        if (modules.router.hashFromRoute(modules.app.App.route) !== location.hash) {
          window.__navigationHarness.navigate(modules.router.routeFromHash(location.hash));
        }
      });
      window.__navigationHashObserver = true;
    }
    document.querySelector("#inbox-list").innerHTML = '<div style="padding:20px"><strong>Projects</strong><p>Build</p><p>Docs</p></div>';
    document.querySelector("#root").innerHTML = '<div style="padding:20px"><h2>README.md</h2><p>Cached navigation evidence fixture.</p></div>';
    directoryRail.paintDirectoryRail(document.querySelector("#dir-rail"), { active: "files", onSelect() {} });
    document.querySelector("#agent-rail").innerHTML = '<div class="rail-panel" id="rail-panel"><div class="rail-head" style="padding:16px">Conversation</div><div class="rail-body"><p style="padding:16px">Cached discussion</p></div></div><div class="rail-strip"></div>';
    document.body.classList.add("agent-open");
    await feed.startFeed(); await toolbar.initToolbar();
    document.querySelectorAll("[data-select]").forEach((control) => control.addEventListener("click", () => {
      window.__navigationHarness.focusInGesture = document.activeElement?.matches(".tb-filter") === true;
    }));
  }, { theme, longNames, kind, duplicateNames });
  await page.locator(".toolbar").waitFor();
  if (kind === "workspace") await page.locator("[data-workspace-lock]").waitFor();
  await page.evaluate(() => document.fonts.ready);
  if (["workspaceTask", "projectTask"].includes(kind)) {
    await page.waitForFunction(() => {
      const leaf = document.querySelector(".tb-leaf");
      return !leaf || leaf.textContent.includes("542");
    });
  }
}
