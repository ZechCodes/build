import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

for (const scope of ["device", "project", "workspace"]) {
  it(`keeps the ${scope} editor usable at phone width with cached data and keyboard input`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountLayout(page, '<main id="editor" style="padding:16px"></main>', { basePath });
      await loadBrowserModules(page, { editor: "src/core/scopedCompactionEditor.js", cache: "src/core/localCache.js" }, basePath);
      await page.evaluate(async (scope) => {
        const { editor, cache } = window.__layoutModules;
        const kinds = { device: "settings", project: "project-settings", workspace: "workspace-settings" };
        const parent = { device: "default", project: "device", workspace: "project" }[scope];
        const providers = ["claude", "claude_adk", "codex", "codex_app_server", "pi"];
        window.__scopeAddress = { deviceId: "device", entityId: scope === "device" ? "" : "owner", kind: kinds[scope] };
        window.__settings = {
          compaction_thresholds: { claude: 200000, codex: 150000 },
          compaction: {
            inherited: Object.fromEntries(providers.map((id) => [id, { tokens: 150000, source: { kind: parent } }])),
            effective: Object.fromEntries(providers.map((id) => [id, { tokens: id === "claude" ? 200000 : 150000, source: { kind: parent } }])),
          },
        };
        await cache.writeCached(window.__scopeAddress, window.__settings);
        await cache.writeCached({ deviceId: "device", entityId: "", kind: "compaction-support" }, { harnessThresholds: true });
        document.querySelector("#editor").innerHTML = editor.scopedCompactionEditorHtml({ scope });
        window.__calls = [];
        window.__editor = editor.mountScopedCompactionEditor(document.querySelector("#editor"), {
          scope, ownerId: "owner", deviceId: "device", callRpc: (...args) => {
            window.__calls.push(args);
            return new Promise(() => {});
          },
        });
        await window.__editor.read();
      }, scope);
      const input = page.locator('[data-compaction-harness="claude"] input');
      await input.fill("175000");
      await page.evaluate(async () => {
        const input = document.activeElement;
        window.__focusedInput = input;
        input.setSelectionRange(1, 4);
        const { cache } = window.__layoutModules;
        await cache.writeCached(window.__scopeAddress, { ...window.__settings, compaction_thresholds: { claude: 250000 } });
        await cache.writeCached({ deviceId: "device", entityId: "", kind: "compaction-support" }, { harnessThresholds: true });
        await window.__editor.read();
      });
      expect(await input.inputValue()).toBe("175000");
      expect(await page.evaluate(() => document.activeElement === window.__focusedInput)).toBe(true);
      expect(await page.evaluate(() => [document.activeElement.selectionStart, document.activeElement.selectionEnd])).toEqual([1, 4]);
      expect(await page.locator('[data-compaction-harness="claude"] [data-compaction-inherit]').innerText()).toContain("150k tokens");
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await captureLayout(page, `compaction-${scope}-narrow.png`);
      await input.press("Enter");
      await page.waitForFunction(() => window.__calls.length === 1);
      expect(await page.evaluate(() => window.__calls[0][1].compaction_thresholds)).toEqual({ claude: 175000 });
      await page.evaluate(() => window.__editor.dispose());
    }, { width: 360, height: 1100 });
  });
}

it("renders the older-bridge explanation and prevents new writes", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountLayout(page, '<main id="editor" style="padding:20px;max-width:650px"></main>', { basePath });
    await loadBrowserModules(page, { editor: "src/core/scopedCompactionEditor.js", cache: "src/core/localCache.js" }, basePath);
    await page.evaluate(async () => {
      const { editor, cache } = window.__layoutModules;
      await cache.writeCached({ deviceId: "device", entityId: "", kind: "settings" }, { compact_above_tokens: 200000 });
      await cache.writeCached({ deviceId: "device", entityId: "", kind: "compaction-support" }, { harnessThresholds: false });
      document.querySelector("#editor").innerHTML = editor.scopedCompactionEditorHtml({ scope: "device" });
      window.__editor = editor.mountScopedCompactionEditor(document.querySelector("#editor"), {
        scope: "device", deviceId: "device", callRpc: () => { throw new Error("Old bridge must not receive new fields"); },
      });
      await window.__editor.read();
    });
    expect(await page.locator("[data-compaction-support]").innerText()).toBe("Update the bridge to set compaction defaults by harness.");
    expect(await page.locator("[data-compaction-save]:disabled").count()).toBe(5);
    await captureLayout(page, "compaction-older-bridge.png");
    await page.evaluate(() => window.__editor.dispose());
  }, { width: 720, height: 1100 });
});
