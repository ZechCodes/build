import { loadBrowserModules } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";

// Production toolbar, inbox and mounted conversation, with only cached fixture
// data and a stubbed agent call. No device session or bridge is opened.
export async function mountAgentHeader(page, basePath, { theme = "light" } = {}) {
  await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "project", inboxHeader: true });
  await page.evaluate(() => { window.__headerNavigation = window.__layoutModules; });
  await loadBrowserModules(page, { rail: "src/core/agentRail.js" }, basePath);
  await page.evaluate(async () => {
    Object.assign(window.__layoutModules, window.__headerNavigation);
    const { rail, cache, inbox } = window.__layoutModules;
    await inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false });
    await cache.writeCached({ deviceId: "nav-device", entityId: "build", kind: "row" }, {
      kind: "project", entity_id: "build", project_id: "build", agents: [{
        id: "header-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Build", topic: "Bridge roll and #564", model: "claude-opus-5",
        watched: true, working: false, unread_count: 0, has_terminal: false,
      }],
    });
    window.__headerRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: "nav-device", projectId: "build", entityId: "build",
      openAgentId: "header-agent", panelOpen: true,
      call: async (method) => method === "models.list" ? { providers: [] } : { items: [] },
    });
  });
  await page.waitForSelector('.rail-head [data-harness-icon="claude_adk"]');
  await page.waitForFunction(() => document.querySelector(".rail-who-work")?.textContent === "Bridge roll and #564");
  const pin = page.locator(".rail-head .pinbtn");
  if (await pin.getAttribute("aria-pressed") === "false") await pin.click();
  await page.waitForFunction(() => !document.querySelector("#agent-rail").classList.contains("rail-popover"));
  await page.evaluate(() => {
    document.querySelector("#inbox-open").classList.add("has-attention");
    document.querySelector(".inbox-open-count").textContent = "2";
  });
  await page.evaluate(() => document.fonts.ready);
}
