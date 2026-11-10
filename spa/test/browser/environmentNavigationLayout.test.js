import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

async function mountEnvironments(page, basePath, theme) {
  await mountNavigationBreadcrumbs(page, basePath, { duplicateNames: true, theme });
  await page.evaluate(() => { window.__navigationModules = window.__layoutModules; });
  await loadBrowserModules(page, { theme: "src/core/theme.js" }, basePath);
  await page.evaluate(theme => {
    Object.assign(window.__layoutModules, window.__navigationModules);
    delete window.__navigationModules;
    window.__layoutModules.theme.applyTheme(theme);
  }, theme);
  await page.evaluate(async () => {
    const { cache } = window.__layoutModules;
    const address = kind => ({ deviceId: "nav-device", entityId: "", kind });
    const held = (await cache.readCached(address("projects"))).value;
    const projects = held.map(project => ({ ...project, environment_id: "shared", last_activity_ms: Date.now() }));
    await cache.writeCached(address("projects"), projects);
    await cache.writeCached(address("environments"), [{ environment_id: "shared", name: "Shared environment with a very long name", project_ids: ["build", "docs"] }]);
    await cache.writeCached(address("environment-support"), { list: true, createProject: true });
  });
  await page.locator(".tb-environment").waitFor({ state: "attached" });
}

it.each([1280, 761, 760, 390, 320].flatMap(width => ["light", "dark"].map(theme => ({width, theme}))))("environment badges retain control geometry at $width px in $theme", async ({width, theme}) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountEnvironments(page, basePath, theme);
    const measured = await page.evaluate(() => {
      const rect = selector => document.querySelector(selector).getBoundingClientRect();
      const env = document.querySelector(".tb-environment");
      const glyph = rect('[data-project-return] [data-kind="project"]');
      return { envVisible: getComputedStyle(env).display !== "none", envOverflow: env.scrollWidth > env.clientWidth,
        glyph: [glyph.width, glyph.height], arrow: rect(".tb-switch").height,
        workspace: rect('[data-select="workspace"]').height, bar: rect("#toolbar").height, scrollWidth: document.documentElement.scrollWidth };
    });
    expect(measured.envVisible).toBe(width > 760);
    if (width > 760) expect(measured.envOverflow).toBe(true);
    expect(measured.glyph).toEqual([16, 16]);
    expect(measured.arrow).toBe(44);
    expect(measured.workspace).toBe(44);
    expect(measured.bar).toBe(44);
    expect(measured.scrollWidth).toBeLessThanOrEqual(width);
    await page.locator('[data-select="project"]').click();
    await expect.poll(() => page.locator('[data-environment-group="nav-device/shared"]').count()).toBe(1);
    expect(await page.locator('[data-environment-group="nav-device/shared"]').textContent()).toContain("Workshop");
    const accessible = await page.locator('[data-environment-group="nav-device/shared"] [data-kind]').getAttribute("aria-hidden");
    expect(accessible).toBe("true");
    const projectRow = page.locator('[data-project="nav-device/build"]:not([data-visit])');
    expect((await projectRow.textContent()).replace(/\s+/g,' ').trim()).toBe('Build Workshop');
    expect(await projectRow.ariaSnapshot()).toContain('link "Build Workshop"');
    const geometry = await page.evaluate(() => {
      const badges = [...document.querySelectorAll('.identity-badge')].filter(node => node.getBoundingClientRect().width);
      const measure = () => badges.map(node => {
        const rect = node.getBoundingClientRect(), row = node.parentElement.getBoundingClientRect();
        const next = node.nextElementSibling?.getBoundingClientRect();
        return [rect.x, rect.y, rect.width, rect.height, row.width, row.height, next?.x, next?.y];
      });
      const colors = badges.map(node => {
        const letters = node.querySelector('.identity-letters');
        const style = getComputedStyle(letters, '::before');
        return { background:getComputedStyle(node.querySelector('rect')).fill, ink:style.color, fontSize:style.fontSize,
          textTransform:style.textTransform, letterSpacing:style.letterSpacing, glyph:style.content, expectedGlyph:`"${letters.dataset.glyph}"`, text:node.textContent };
      });
      const badgeGeometry = measure();
      const contents = badges.map(node => node.innerHTML);
      badges.forEach(node => { node.innerHTML = '<circle cx="8" cy="8" r="4" fill="currentColor"/>'; });
      const oldGeometry = measure();
      badges.forEach((node, index) => { node.innerHTML = contents[index]; });
      return { badgeGeometry, oldGeometry, colors };
    });
    expect(geometry.badgeGeometry.length).toBeGreaterThan(2);
    expect(geometry.badgeGeometry).toEqual(geometry.oldGeometry);
    for (const colors of geometry.colors) {
      expect(colors.background).not.toBe(colors.ink);
      expect(colors.fontSize).toBe('8px');
      expect(colors.glyph).toBe(colors.expectedGlyph);
      expect(colors.text).toBe('');
      expect(colors.textTransform).toBe('none');
      expect(colors.letterSpacing).toBe('normal');
    }
    await captureLayout(page, `environment-switcher-${width}-${theme}.png`);
  }, { width, height: 844, hasTouch: width < 900 });
}, 45_000);
