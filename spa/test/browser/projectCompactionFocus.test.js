import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

async function openProjectSheet(page, basePath) {
  await mountLayout(page, '<div id="scrim" class="scrim"><div id="sheet" class="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, { sheet: "src/sheets/projectSettings.js", cache: "src/core/localCache.js",
    support: "src/core/sourceEditSupport.js" }, basePath);
  await page.evaluate(async () => {
    const { sheet, cache, support } = window.__layoutModules;
    const providers = ["claude", "claude_adk", "codex", "codex_app_server", "pi"];
    const inherited = { tokens: 200000, source: { kind: "device" } };
    const project = {
      project_id: "project", name: "Project", path: "/project", sources: [],
      isolation: null, isolation_default: "worktree", isolation_effective: "worktree", isolation_available: { rift: false },
      compaction_thresholds: { codex: 150000 },
      compaction: {
        inherited: Object.fromEntries(providers.map((provider) => [provider, inherited])),
        effective: Object.fromEntries(providers.map((provider) => [provider, provider === "codex"
          ? { tokens: 150000, source: { kind: "project", id: "project" } } : inherited])),
      },
    };
    const address = { deviceId: "device", entityId: "project", kind: "project-settings" };
    window.__project = project;
    window.__projectAddress = address;
    window.__compactionCalls = [];
    await cache.writeCached(address, project);
    await cache.writeCached(support.sourceEditSupportAddress("device"), { editsSources: true });
    await cache.writeCached({ deviceId: "device", entityId: "", kind: "compaction-support" }, { harnessThresholds: true });
    window.__projectSheet = sheet.openProjectSettings("project", {
      deviceId: "device", callRpc: async (method, params) => {
        if (method === "project.list") return { projects: [window.__project] };
        if (method !== "project.set_compaction") return {};
        window.__compactionCalls.push(params);
        const answer = structuredClone(window.__project);
        const tokens = params.compaction_thresholds.codex;
        if (tokens === null) delete answer.compaction_thresholds.codex;
        else answer.compaction_thresholds.codex = tokens;
        answer.compaction.effective.codex = tokens === null ? answer.compaction.inherited.codex
          : { tokens, source: { kind: "project", id: "project" } };
        window.__project = answer;
        return answer;
      },
    });
    await window.__projectSheet.whenCachePainted();
  });
  await page.waitForFunction(() => document.querySelector('[data-compaction-harness="codex"] input')?.value === "150000"
    && !document.querySelector('[data-compaction-harness="codex"] [data-compaction-inherit]')?.disabled);
  // Let the initial independent support read and project pull finish painting.
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

for (const update of ["project push", "late greeting"]) {
  for (const action of ["inherit", "save"]) {
    it(`keeps the full project sheet's ${action} button focused through a ${update} and activates it with Enter`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await openProjectSheet(page, basePath);
        const row = page.locator('[data-compaction-harness="codex"]');
        if (action === "save") await row.locator("input").fill("175000");
        const button = row.locator(`[data-compaction-${action}]`);
        await button.focus();
        await page.evaluate(async (update) => {
          const { cache, support } = window.__layoutModules;
          window.__focusedCompactionButton = document.activeElement;
          window.__previousProjectFrame = document.querySelector("#sheet").firstElementChild;
          if (update === "project push") {
            window.__project = { ...window.__project, name: "Project updated elsewhere" };
            await cache.writeCached(window.__projectAddress, window.__project);
          } else {
            await cache.writeCached(support.sourceEditSupportAddress("device"), { editsSources: false, syncsBase: true });
          }
        }, update);
        await page.waitForFunction(() => document.querySelector("#sheet").firstElementChild !== window.__previousProjectFrame);
        expect(await button.evaluate((element) => element === window.__focusedCompactionButton)).toBe(true);
        expect(await page.evaluate(() => document.activeElement === window.__focusedCompactionButton)).toBe(true);
        await page.keyboard.press("Enter");
        await page.waitForFunction(() => window.__compactionCalls.length === 1);
        expect(await page.evaluate(() => window.__compactionCalls[0])).toEqual({
          project_id: "project", compaction_thresholds: { codex: action === "inherit" ? null : 175000 },
        });
        await page.waitForFunction(() => document.querySelector('[data-compaction-harness="codex"] [role="status"]').textContent.includes("Saved."));
        expect(await row.locator("input").inputValue()).toBe(action === "inherit" ? "" : "175000");
        expect(await row.locator("[data-compaction-effective]").innerText()).toBe(action === "inherit" ? "Effective: 200k tokens" : "Effective: 175k tokens");
        expect(await row.locator("[data-compaction-inherit]").innerText()).toBe("Inherit — 200k tokens from device");
        await page.locator("#pscancel").click();
      });
    }, 30_000);
  }
}
