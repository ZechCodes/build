import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

const html = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];

// Cached fixtures and an in-memory device session; no bridge or account.
export async function mountProjectsFace(page, basePath) {
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js",
    inbox: "src/core/inboxShell.js", view: "src/core/inboxView.js", contexts: "src/core/deviceContexts.js",
  }, basePath);
  await page.evaluate(async () => {
    const { app, cache, feed, inbox, view, contexts } = window.__layoutModules;
    const deviceId = "projects-review";
    app.App.devices = [{ id: deviceId, name: "Workshop", status: "online" }];
    app.App.selectedDeviceId = deviceId;
    app.App.route = { name: "project", deviceId, projectId: "build" };
    const now = Date.now();
    const hour = 3_600_000;
    const project = (id, name, age) => ({ id, name, deviceId, projectKey: `${deviceId}/${id}`,
      session_started_ms: now - age, last_activity_ms: now - age });
    const projects = [project("zulu", "Zulu", hour * 3), project("build", "Build", hour),
      project("alpha", "Alpha", hour * 24 * 30), project("bravo", "Bravo", 0)];
    const workspace = (id, name, projectId, age) => ({ id, name, project_id: projectId, deviceId,
      projectKey: `${deviceId}/${projectId}`, workspaceKey: `${deviceId}/${id}`,
      status: "ready", managed: true, entity_id: id, directories: [{ id: "src", is_git: true }],
      session_started_ms: now - age, last_activity_ms: now - age });
    const workspaces = [workspace("zulu-live", "Running work", "zulu", hour * 3),
      workspace("build-live", "Current work", "build", hour),
      workspace("build-quiet", "Older workspace", "build", hour * 48),
      workspace("alpha-quiet", "Last month's work", "alpha", hour * 24 * 30)];
    const items = workspaces.map((workspace) => ({ kind: "branch", run_id: workspace.entity_id,
      project_id: workspace.project_id, deviceId, projectKey: workspace.projectKey,
      agents: [{ id: `${workspace.id}-agent`, watched: true, working: workspace.id === "zulu-live",
        unread_count: workspace.id === "build-live" ? 2 : 0 }] }));
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { projects, workspaces, items, runs: items });
    contexts.adoptDeviceSession({ deviceId, call: async () => ({}), close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await inbox.initInboxRail();
    await feed.startFeed();
    view.setInboxView("projects");
    if (innerWidth <= 900) document.body.classList.add("inbox-collapsed", "inbox-popover-open");
  });
  await page.locator('.inbox-project[data-project="projects-review/build"]').waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(0, 0);
}

export async function inspectProjectsFace(page) {
  return page.locator(".inbox-project").evaluateAll((blocks) => blocks.map((block) => {
    const style = getComputedStyle(block);
    return { name: block.querySelector(".inbox-project-name").textContent.trim(),
      active: block.classList.contains("active"), background: style.backgroundColor,
      border: style.borderWidth, shadow: style.boxShadow };
  }));
}
