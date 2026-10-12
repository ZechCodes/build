import { expect, it } from "vitest";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { shellHtml, SHELL_STYLES } from "./chatMenuSeed.mjs";

it("scopes cached Claude limits to the selected session in the production rail", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    page.setDefaultTimeout(5000);
    await mountLayout(page, shellHtml("phone"), { basePath, styles: SHELL_STYLES });
    await loadChatOverviewModules(page, basePath, { limits: "src/core/usageLimits.js" });
    await page.evaluate(async () => {
      const { app, cache, limits, rail, merge } = window.__layoutModules;
      app.App.devices = [{ id: "limit-device", name: "Fixture computer" }];
      app.App.selectedDeviceId = "limit-device";
      await cache.writeCached({ deviceId: "limit-device", entityId: "limit-run", kind: "row" }, {
        kind: "workspace", entity_id: "limit-run", workspace_id: "limit-workspace", project_id: "limit-project",
        agents: [
          { id: "codex-worker", ordinal: 1, provider: "codex_app_server", active_model: "gpt-6-astra", topic: "Take over orchestration" },
          { id: "claude-worker", ordinal: 2, provider: "claude", topic: "Claude worker" },
        ].map((agent) => ({ ...agent, state: "live", watched: true, unread_count: 0 })),
      });
      await cache.writeCached({ deviceId: "limit-device", entityId: "", kind: "workspaces" }, [
        merge.stampWorkspace({ id: "limit-workspace", project_id: "limit-project", entity_id: "limit-run", name: "Banner scope" }, "limit-device"),
      ]);
      await limits.writeUsageLimits("limit-device", [{ harness: "claude_adk", resets_at: null, said: "You've hit your session limit" }]);
      window.__limitRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "workspace", deviceId: "limit-device", projectId: "limit-project", workspaceId: "limit-workspace", entityId: "limit-run",
        openAgentId: "codex-worker", panelOpen: true,
        call: async (method) => method === "models.list" ? { providers: [] } : { items: [] },
      });
    });
    await page.waitForSelector(".rail-head .agent-model-badge");
    expect(await page.locator(".usage-limit-banner").count()).toBe(0);
    await captureLayout(page, "573-codex-no-claude-banner.png");
    await page.locator('[data-bubble="agent"][data-agent="claude-worker"]').click();
    await page.waitForSelector(".usage-limit-banner");
    expect(await page.locator(".usage-limit-banner").textContent()).toContain("Claude session limit reached");
    await captureLayout(page, "573-claude-banner.png");
    await page.locator('[data-bubble="agent"][data-agent="codex-worker"]').click();
    await page.waitForSelector(".usage-limit-banner", { state: "detached" });
    await page.evaluate(() => window.__limitRail.dispose());
  }, { width: 390, height: 844 });
}, 30_000);
