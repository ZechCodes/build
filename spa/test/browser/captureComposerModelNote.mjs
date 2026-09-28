// Capture the composer's model menu for #205, open at phone width in both
// themes, on a catalog whose CLI is too old for one model: the models it can
// run, then the "Update Claude Code" note at the menu's foot. The production
// composer and model menu are mounted into a phone-sized panel.
// Run from spa/: node test/browser/captureComposerModelNote.mjs [output dir]
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const output = process.argv[2] || "/tmp/composer-model-note";
await mkdir(output, { recursive: true });

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{
    id: "claude_adk",
    label: "Claude Code",
    cli_name: "Claude Code",
    cli_version: "2.1.280",
    models: [
      ["claude-fable-5-1", "Claude Fable 5.1"], ["claude-opus-5-5", "Claude Opus 5.5"],
      ["claude-opus-5", "Claude Opus 5"], ["claude-opus-4-8", "Claude Opus 4.8"],
      ["claude-sonnet-5", "Claude Sonnet 5"], ["claude-sonnet-4-6", "Claude Sonnet 4.6"],
      ["claude-haiku-4-5", "Claude Haiku 4.5"],
    ].map(([id, label]) => ({ id, label, supports_effort: true })),
    efforts: ["low", "medium", "high"],
    unavailable: [{ id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", requires_cli: "2.1.284" }],
  }],
};

for (const theme of ["dark", "light"]) {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div id="host" style="position:absolute;left:12px;right:12px;bottom:40px"></div>', { basePath });
    await loadBrowserModules(page, { composer: "src/core/composer.js" }, basePath);
    await page.evaluate(({ theme, catalog }) => {
      document.documentElement.dataset.theme = theme;
      const { composerHtml, mountComposerModelMenu } = window.__layoutModules.composer;
      const host = document.querySelector("#host");
      const ids = { input: "ti", send: "ts", hint: "th" };
      host.innerHTML = composerHtml({ inputId: ids.input, sendId: ids.send, hintId: ids.hint, placeholder: "Start an agent here…", modelMenu: true });
      mountComposerModelMenu(host, { ids, onChoose: () => {} })
        .set(catalog, "claude_adk", { provider: "claude_adk", model: "", effort: "" });
    }, { theme, catalog: CATALOG });
    await page.locator(".composer-model .caret").click();
    await page.waitForTimeout(400);
    await page.screenshot({ path: join(output, `composer-model-note-${theme}-phone.png`) });
    // From the keyboard, the last row scrolls into sight above the pinned note.
    await page.locator(".composer-model .caret").focus();
    await page.keyboard.press("ArrowUp");
    await page.waitForTimeout(400);
    const { rowBottom, noteTop } = await page.evaluate(() => ({
      rowBottom: document.activeElement.getBoundingClientRect().bottom,
      noteTop: document.querySelector(".composer-model .menu-note").getBoundingClientRect().top,
    }));
    if (rowBottom > noteTop + 0.5) throw new Error(`last row ends at ${rowBottom}, under the note at ${noteTop}`);
    await page.screenshot({ path: join(output, `composer-model-note-${theme}-phone-keyboard.png`) });
  }, { width: 390, height: 844, deviceScaleFactor: 2 });
}
console.log(`wrote ${output}`);
