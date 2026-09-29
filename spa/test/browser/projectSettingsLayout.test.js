// #228: Project settings is General, Sources, Isolation and Danger zone, and
// each source is one card holding its label, folder, base branch, remote and
// its own Remove and Save. Checked in Chromium against the production renderer
// and styles: every section heading starts at the body's left edge (the old
// "Workspace folders" legend sat off it), and each card's controls stay inside
// that card rather than floating between sources.
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const PROJECT = {
  project_id: "proj-4",
  name: "Do",
  path: "/home/zech/Projects/Do-sources/Do",
  base_branch: "main",
  is_git: true,
  remote: "git@github.com:ZechCodes/Do.git",
  isolation: null,
  isolation_default: "rift",
  isolation_effective: "rift",
  isolation_available: { rift: true },
  sources: ["Do", "Airlock", "Aviary", "Alexandria"].map((name, index) => ({
    id: `source-${index + 1}`,
    name,
    mount: name,
    path: `/home/zech/Projects/Do-sources/${name}`,
    is_git: true,
    base_branch: "main",
    remote: `git@github.com:ZechCodes/${name}.git`,
  })),
};

for (const { label, width, height } of [
  { label: "desktop", width: 1280, height: 900 },
  { label: "mobile", width: 390, height: 800 },
]) {
  it(`lays out the project's sections and source cards on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, '<div id="scrim" class="scrim"><div id="sheet" class="sheet"></div></div>', { basePath });
      await loadBrowserModules(page, {
        sheet: "src/sheets/projectSettings.js",
        cache: "src/core/localCache.js",
        support: "src/core/sourceEditSupport.js",
      }, basePath);
      await page.evaluate(async (project) => {
        const { sheet, cache, support } = window.__layoutModules;
        await cache.writeCached(support.sourceEditSupportAddress("laptop"), { editsSources: true });
        sheet.openProjectSettings(project.project_id, {
          deviceId: "laptop",
          callRpc: async (method) => (method === "project.list" ? { projects: [project] } : project),
        });
      }, PROJECT);
      await page.waitForFunction(() =>
        document.querySelector('.ps-source[data-source-id="source-2"] input[data-field="name"]')?.readOnly === false);

      const layout = await page.evaluate(() => {
        const body = document.querySelector(".settings-sheet-body").getBoundingClientRect();
        const padding = parseFloat(getComputedStyle(document.querySelector(".settings-sheet-body")).paddingLeft);
        const headings = [...document.querySelectorAll(".ps-section > h4")].map((heading) => ({
          text: heading.textContent,
          left: heading.getBoundingClientRect().left,
        }));
        const cards = [...document.querySelectorAll(".ps-source")].map((card) => {
          const box = card.getBoundingClientRect();
          const inside = [...card.querySelectorAll("input, button, .ps-source-tag")].every((element) => {
            const inner = element.getBoundingClientRect();
            return inner.left >= box.left - 0.5 && inner.right <= box.right + 0.5
              && inner.top >= box.top - 0.5 && inner.bottom <= box.bottom + 0.5;
          });
          return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, inside };
        });
        const emptyNotes = [...document.querySelectorAll(".ps-source [data-source-warning], .ps-source [data-source-status]")]
          .filter((note) => note.textContent === "")
          .map((note) => getComputedStyle(note).display);
        return { bodyLeft: body.left + padding, bodyRight: body.right, headings, cards, emptyNotes, viewport: innerWidth };
      });

      expect(layout.headings.map((heading) => heading.text)).toEqual(["General", "Sources", "Isolation", "Danger zone"]);
      for (const heading of layout.headings) expect(Math.abs(heading.left - layout.bodyLeft)).toBeLessThanOrEqual(1);
      expect(layout.cards).toHaveLength(4);
      expect(layout.emptyNotes).toHaveLength(8);
      expect(new Set(layout.emptyNotes)).toEqual(new Set(["none"]));
      for (const card of layout.cards) {
        expect(card.inside).toBe(true);
        expect(Math.abs(card.left - layout.bodyLeft)).toBeLessThanOrEqual(1);
        expect(card.right).toBeLessThanOrEqual(layout.viewport);
      }
      for (let index = 1; index < layout.cards.length; index += 1) {
        expect(layout.cards[index].top).toBeGreaterThanOrEqual(layout.cards[index - 1].bottom);
      }
      await captureLayout(page, `project-settings-${label}.png`);
      await page.evaluate(() => document.querySelector('.ps-source[data-source-id="source-2"]').scrollIntoView());
      await captureLayout(page, `project-settings-${label}-sources.png`);
    }, { width, height });
  }, 30_000);
}
