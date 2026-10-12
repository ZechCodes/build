import { expect, it } from "vitest";
import { captureLayout, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { loadChatOverviewModules, seedChatOverview } from "./chatOverviewSeed.mjs";
import { shellHtml, SHELL_STYLES } from "./chatMenuSeed.mjs";

it("scopes cached Claude limits to the selected session in the production rail", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, shellHtml("desktop"), { basePath, styles: SHELL_STYLES });
    await loadChatOverviewModules(page, basePath, { limits: "src/core/usageLimits.js" });
    await page.evaluate(seedChatOverview);
    await page.waitForSelector('[data-bubble="agent"][data-agent="current-agent"]');
    await page.evaluate(async () => {
      const { cache, limits } = window.__layoutModules;
      const address = { deviceId: "overview-device", entityId: "current-run", kind: "row" };
      const row = (await cache.readCached(address)).value;
      await cache.writeCached(address, { ...row, agents: [
        { ...row.agents[0], provider: "codex_app_server", active_model: "gpt-6-astra" },
        { ...row.agents[0], id: "claude-worker", ordinal: 2, provider: "claude", topic: "Claude worker" },
      ] });
      await limits.writeUsageLimits("overview-device", [{ harness: "claude_adk", resets_at: null, said: "You've hit your session limit" }]);
    });
    await page.locator('[data-bubble="agent"][data-agent="current-agent"]').click();
    await page.waitForSelector(".rail-head .agent-model-badge");
    expect(await page.locator(".usage-limit-banner").count()).toBe(0);
    await captureLayout(page, "573-codex-no-claude-banner.png");
    await page.locator('[data-bubble="agent"][data-agent="claude-worker"]').click();
    await page.waitForSelector(".usage-limit-banner");
    expect(await page.locator(".usage-limit-banner").textContent()).toContain("Claude session limit reached");
    await captureLayout(page, "573-claude-banner.png");
    await page.locator('[data-bubble="agent"][data-agent="current-agent"]').click();
    await page.waitForSelector(".usage-limit-banner", { state: "detached" });
    await page.evaluate(() => window.__overviewRail.dispose());
  }, { width: 390, height: 844 });
}, 30_000);
