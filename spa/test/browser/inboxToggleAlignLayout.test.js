import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const shellMarkup = readFileSync(resolve("index.html"), "utf8")
  .match(/<body>([\s\S]*)<\/body>/)[1]
  .replace('<div id="toolbar"></div>', `<div id="toolbar"><div class="toolbar">
    <button class="tb-sel tb-project" type="button"><span class="tb-name">Build</span></button>
    <button class="tb-sel" type="button">Tasks</button></div></div>`)
  // A project has an agent strip occupying the desktop grid's third column.
  .replace('<aside id="agent-rail" aria-label="Agents"></aside>',
    '<aside id="agent-rail" class="rail-collapsed" aria-label="Agents"><div class="rail-strip"></div></aside>');

const settleLayout = (page) => page.evaluate(() => new Promise((resolve) => {
  requestAnimationFrame(() => requestAnimationFrame(resolve));
}));

async function mountCollapsedShell(page, basePath) {
  await mountLayout(page, shellMarkup, { basePath });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await loadBrowserModules(page, { directory: "src/core/directoryRail.js" }, basePath);
  await page.evaluate(() => {
    const { paintDirectoryRail, PROJECT_TABS } = window.__layoutModules.directory;
    paintDirectoryRail(document.querySelector("#dir-rail"), {
      tabs: PROJECT_TABS, active: "tasks", onSelect: () => {}, sidebar: false,
    });
    document.body.classList.add("inbox-collapsed");
  });
  await settleLayout(page);
}

const geometry = (page) => page.evaluate(() => {
  const box = (selector) => {
    const { x, width, right } = document.querySelector(selector).getBoundingClientRect();
    return { x, width, right, center: x + width / 2 };
  };
  return {
    toggle: box("#inbox-open svg"), directory: box("#dir-rail .dirtab svg"),
    button: box("#inbox-open"), badge: box(".inbox-open-count"),
    toolbar: box(".toolbar"), project: box(".tb-project"),
    toolbarPadding: getComputedStyle(document.querySelector(".toolbar")).paddingLeft,
  };
});

function expectAligned({ toggle, directory, button, toolbarPadding, project, toolbar }, clearance = 48) {
  expect(toggle.width).toBeGreaterThan(0);
  expect(directory.width).toBeGreaterThan(0);
  expect(Math.abs(toggle.center - directory.center)).toBeLessThanOrEqual(1);
  expect(toolbarPadding).toBe(`${clearance}px`);
  expect(project.x - toolbar.x).toBe(clearance);
  expect(project.x).toBeGreaterThan(button.right);
}

// Both grid arrangements, and both sides of the inbox overlay breakpoint.
for (const width of [1280, 901, 900, 761, 760, 390]) {
  it(`aligns the collapsed inbox toggle with the directory icons at ${width}px`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountCollapsedShell(page, basePath);
      const beforeBadge = await geometry(page);
      await captureLayout(page, `inbox-toggle-collapsed-${width}.png`);
      expectAligned(beforeBadge);

      await page.evaluate(() => {
        document.querySelector("#inbox-open").classList.add("has-attention");
        document.querySelector(".inbox-open-count").textContent = "99+";
      });
      await settleLayout(page);
      const afterBadge = await geometry(page);
      expectAligned(afterBadge);
      expect(afterBadge.project).toEqual(beforeBadge.project);
      expect(afterBadge.badge.right).toBeLessThanOrEqual(afterBadge.project.x);
      expect(await page.locator(".inbox-open-count").isVisible()).toBe(true);
    }, { width, height: 820 });
  });
}

it("keeps the toggle centered when the shared rail and toggle sizes change", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountCollapsedShell(page, basePath);
    await page.evaluate(() => {
      document.documentElement.style.setProperty("--dir-rail", "48px");
      document.documentElement.style.setProperty("--inbox-toggle", "32px");
    });
    await settleLayout(page);
    expectAligned(await geometry(page));
  });
});

it("aligns with the standing rail while preserving toolbar safe-area clearance", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const session = await page.context().newCDPSession(page);
    await session.send("Emulation.setSafeAreaInsetsOverride", { insets: { left: 20 } });
    await mountCollapsedShell(page, basePath);
    expectAligned(await geometry(page), 68);
  }, { width: 390, height: 820 });
});
