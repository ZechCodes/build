import { loadBrowserModules } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";

// Production toolbar, inbox and mounted conversation, with only cached fixture
// data and a stubbed agent call. No device session or bridge is opened.
export async function mountAgentHeader(page, basePath, { theme = "light", kind = "project", pinned } = {}) {
  await mountNavigationBreadcrumbs(page, basePath, { theme, kind, inboxHeader: true });
  await page.evaluate(() => { window.__headerNavigation = window.__layoutModules; });
  await loadBrowserModules(page, { rail: "src/core/agentRail.js" }, basePath);
  await page.evaluate(async (kind) => {
    Object.assign(window.__layoutModules, window.__headerNavigation);
    const { rail, cache, inbox } = window.__layoutModules;
    await inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false });
    const entityId = kind === "workspace" ? "current" : "build";
    if (kind === "workspace") {
      const address = { deviceId: "nav-device", entityId: "", kind: "workspaces" };
      const workspaces = (await cache.readCached(address)).value;
      await cache.writeCached(address, workspaces.map((workspace) => workspace.id === "current"
        ? { ...workspace, entity_id: entityId } : workspace));
    }
    await cache.writeCached({ deviceId: "nav-device", entityId, kind: "row" }, {
      kind, entity_id: entityId, project_id: "build", workspace_id: kind === "workspace" ? "current" : null, agents: [{
        id: "header-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Build", topic: "Bridge roll and #564", model: "claude-opus-5",
        watched: true, working: false, unread_count: 0, has_terminal: false,
      }],
    });
    window.__headerRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
      kind, deviceId: "nav-device", projectId: "build", entityId,
      workspaceId: kind === "workspace" ? "current" : null,
      openAgentId: "header-agent", panelOpen: true,
      call: async (method) => method === "models.list" ? { providers: [] } : { items: [] },
    });
  }, kind);
  await page.waitForSelector('.rail-head [data-harness-icon="claude_adk"]');
  await page.waitForFunction(() => document.querySelector(".rail-who-work")?.textContent === "Bridge roll and #564");
  // Leave the default intact unless this fixture explicitly exercises pinning.
  if (pinned !== undefined) {
    const pin = page.locator(".rail-head .pinbtn");
    if (await pin.getAttribute("aria-pressed") !== String(pinned)) await pin.click();
    await page.waitForFunction((pinned) => document.querySelector("#agent-rail").classList.contains("rail-popover") !== pinned, pinned);
  }
  await page.evaluate(() => {
    document.querySelector("#inbox-open").classList.add("has-attention");
    document.querySelector(".inbox-open-count").textContent = "2";
  });
  await page.evaluate(() => document.fonts.ready);
}
