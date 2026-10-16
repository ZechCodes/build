import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

const settle = (page) => page.waitForTimeout(450);

const toolbarGeometry = (page) => page.evaluate(() => {
  const bar = document.querySelector("#toolbar").getBoundingClientRect();
  const inset = (selector) => document.querySelector(selector).getBoundingClientRect().left - bar.left;
  return { switcher: inset(".tb-switch"), project: inset(".tb-project"),
    workspaces: inset('[data-select="workspace"]'), height: bar.height };
});

// Observe the real CSS/grid/rail animation, rather than comparing only the
// resulting markup. The toolbar must retain its nodes and focus on every frame.
async function samplePinTransition(page, collapsed) {
  return page.evaluate(async (collapsed) => {
    const toolbar = document.querySelector(".toolbar");
    const nodes = [toolbar, ...toolbar.querySelectorAll("*")];
    const visibleNodes = nodes.filter((node) => getComputedStyle(node).display !== "none");
    for (let parent = toolbar.parentElement; parent; parent = parent.parentElement) visibleNodes.push(parent);
    const controls = [".tb-switch", ".tb-project", '[data-select="workspace"]'].map((selector) => toolbar.querySelector(selector));
    let childChanges = 0;
    const observer = new MutationObserver((records) => { childChanges += records.length; });
    observer.observe(toolbar, { childList: true, subtree: true });
    const frames = [];
    const sample = () => {
      const bar = document.querySelector("#toolbar").getBoundingClientRect();
      const current = [document.querySelector(".toolbar"), ...document.querySelectorAll(".toolbar *")];
      frames.push({
        retained: current.length === nodes.length && nodes.every((node, index) => node === current[index] && node.isConnected),
        visible: visibleNodes.every((node) => {
          const style = getComputedStyle(node);
          return !node.hidden && style.display !== "none" && style.visibility === "visible" && Number(style.opacity) > 0;
        }),
        focus: document.activeElement === controls[0],
        insets: controls.map((control) => control.getBoundingClientRect().left - bar.left),
        sizes: controls.map((control) => {
          const rect = control.getBoundingClientRect();
          return { width: rect.width, height: rect.height };
        }),
      });
    };
    sample();
    await window.__layoutModules.inbox.setInboxCollapsed(collapsed, { animate: true, persist: false, reveal: false });
    const started = performance.now();
    await new Promise((resolve) => {
      const tick = () => {
        sample();
        if (performance.now() - started >= 450) resolve();
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    observer.disconnect();
    return { frames, childChanges };
  }, collapsed);
}

function expectStableTransition({ frames, childChanges }) {
  expect(childChanges).toBe(0);
  expect(frames.length).toBeGreaterThan(2);
  const initial = frames[0].insets;
  const end = frames.at(-1).insets[0];
  expect(frames.some(({ insets }) => insets[0] > Math.min(initial[0], end) + 1
    && insets[0] < Math.max(initial[0], end) - 1)).toBe(true);
  for (const frame of frames) {
    expect(frame.retained).toBe(true);
    expect(frame.visible).toBe(true);
    expect(frame.focus).toBe(true);
    for (const size of frame.sizes) {
      expect(size.width).toBeGreaterThan(0);
      expect(size.height).toBe(44);
    }
    for (let index = 1; index < frame.insets.length; index++) {
      expect(frame.insets[index] - initial[index]).toBeCloseTo(frame.insets[0] - initial[0], 1);
    }
  }
}

for (const width of [901, 1280]) for (const theme of ["light", "dark"]) {
  it(`reclaims the pinned inbox slot with the same toolbar nodes throughout pin/unpin ${width}px ${theme}`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "project", inboxHeader: true });
      // At the desktop breakpoint, leave room for a single row. At 1280px
      // both panels remain open, as in the reported project-page layout.
      if (width === 901) await page.evaluate(() => document.querySelector("#agent-rail").classList.add("rail-collapsed"));
      await page.evaluate(() => window.__layoutModules.inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false }));
      await settle(page);
      const unpinned = await toolbarGeometry(page);
      await captureLayout(page, `pinned-inbox-${width}-${theme}-unpinned.png`);
      await page.locator(".tb-switch").focus();
      const pin = await samplePinTransition(page, false);
      const pinned = await toolbarGeometry(page);
      await captureLayout(page, `pinned-inbox-${width}-${theme}-pinned.png`);
      expect(await page.locator("#inbox-open").isVisible()).toBe(false);
      const unpin = await samplePinTransition(page, true);
      expectStableTransition(pin);
      expectStableTransition(unpin);
      expect(unpinned.switcher).toBe(48);
      expect(pinned.switcher).toBe(0);
      expect(unpinned.project - pinned.project).toBe(48);
      expect(unpinned.workspaces - pinned.workspaces).toBe(48);
      expect(pinned.height).toBe(44);
      expect(await toolbarGeometry(page)).toEqual(unpinned);
      expect(await page.locator("#inbox-open").isVisible()).toBe(true);
    }, { width, height: 800 });
  }, 30_000);
}

for (const width of [390, 900]) {
  it(`keeps the narrow inbox overlay trigger and unpinned spacing ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { kind: "project", inboxHeader: true });
      await settle(page);
      const before = await toolbarGeometry(page);
      expect(before.switcher).toBe(48);
      expect(await page.locator("#inbox-collapse").isVisible()).toBe(false);
      expect(await page.locator("#inbox-open").isVisible()).toBe(true);
      await page.locator("#inbox-open").tap();
      expect(await page.locator("#inbox-rail").isVisible()).toBe(true);
      expect(await page.locator("#inbox-open").isVisible()).toBe(true);
      await page.locator("#inbox-open").tap();
      await settle(page);
      expect(await toolbarGeometry(page)).toEqual(before);
      await captureLayout(page, `pinned-inbox-${width}-narrow.png`);
    }, { width, height: 844, hasTouch: true });
  }, 30_000);
}
