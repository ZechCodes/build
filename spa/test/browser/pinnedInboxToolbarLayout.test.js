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
  const inbox = document.querySelector("#inbox-open svg").getBoundingClientRect();
  return { glyph: inset(".tb-switch svg"), inboxGlyph: inbox.width ? inbox.left - bar.left : null, project: inset(".tb-project"),
    workspaces: inset('[data-select="workspace"]'), height: bar.height };
});

// Observe the real CSS/grid/rail animation, rather than comparing only the
// resulting markup. The toolbar must retain its nodes and focus on every frame.
async function samplePinTransition(page, collapsed, { reverseAfter = null } = {}) {
  return page.evaluate(async ({ collapsed, reverseAfter }) => {
    const toolbar = document.querySelector(".toolbar");
    const nodes = [toolbar, ...toolbar.querySelectorAll("*")];
    const visibleNodes = nodes.filter((node) => getComputedStyle(node).display !== "none");
    for (let parent = toolbar.parentElement; parent; parent = parent.parentElement) visibleNodes.push(parent);
    const controls = [".tb-switch", ".tb-project", '[data-select="workspace"]'].map((selector) => toolbar.querySelector(selector));
    const focused = document.activeElement;
    const scroll = document.querySelector("#console-scroll");
    const scrollTop = scroll?.scrollTop;
    let childChanges = 0;
    const observer = new MutationObserver((records) => { childChanges += records.length; });
    observer.observe(toolbar, { childList: true, subtree: true });
    const frames = [];
    const sample = () => {
      const bar = document.querySelector("#toolbar").getBoundingClientRect();
      const current = [document.querySelector(".toolbar"), ...document.querySelectorAll(".toolbar *")];
      const target = controls[0].getBoundingClientRect();
      const points = [0.5, target.width / 2, target.width - 0.5]
        .map((x) => ({ x: target.left + x, y: target.top + target.height / 2 }));
      const hitNodes = points.map(({ x, y }) => document.elementFromPoint(x, y));
      const pane = document.querySelector("#inbox-rail");
      const paneRect = pane.getBoundingClientRect();
      const paneStyle = getComputedStyle(pane);
      const paneVisible = paneStyle.visibility === "visible" && paneStyle.pointerEvents !== "none";
      frames.push({
        retained: current.length === nodes.length && nodes.every((node, index) => node === current[index] && node.isConnected),
        visible: visibleNodes.every((node) => {
          const style = getComputedStyle(node);
          return !node.hidden && style.display !== "none" && style.visibility === "visible" && Number(style.opacity) > 0;
        }),
        focus: document.activeElement === focused,
        // The existing rail FLIP may cover the bar during unpin. Covered
        // points must stay owned by the pane; uncovered target edges work.
        hit: hitNodes.every((node, index) => {
          const { x, y } = points[index];
          const covered = paneVisible && x > paneRect.left && x < paneRect.right && y > paneRect.top && y < paneRect.bottom;
          return covered ? pane.contains(node) : controls[0].contains(node);
        }),
        paneEdge: !paneVisible || pane.contains(document.elementFromPoint(paneRect.right - 1, paneRect.top + 22)),
        hitNodes: hitNodes.map((node) => node?.id || node?.getAttribute("class") || node?.tagName),
        separate: target.right <= controls[1].getBoundingClientRect().left,
        contained: target.left >= bar.left && target.right <= bar.right,
        content: document.querySelector("#dir-rail").getBoundingClientRect().left === bar.left,
        console: document.querySelector('#console-region[data-size="full"]')?.getBoundingClientRect().left ?? bar.left,
        barLeft: bar.left,
        scroll: !scroll || scroll.scrollTop === scrollTop,
        insets: [controls[0].querySelector("svg"), ...controls.slice(1)].map((control) => control.getBoundingClientRect().left - bar.left),
        sizes: controls.map((control) => {
          const rect = control.getBoundingClientRect();
          return { width: rect.width, height: rect.height };
        }),
      });
    };
    sample();
    await window.__layoutModules.inbox.setInboxCollapsed(collapsed, { animate: true, persist: false, reveal: false });
    if (reverseAfter !== null) setTimeout(() => {
      void window.__layoutModules.inbox.setInboxCollapsed(!collapsed, { animate: true, persist: false, reveal: false });
    }, reverseAfter);
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
  }, { collapsed, reverseAfter });
}

function expectStableTransition({ frames, childChanges }, { animated = true } = {}) {
  expect(childChanges).toBe(0);
  expect(frames.length).toBeGreaterThan(2);
  const initial = frames[0].insets;
  const positions = frames.map(({ insets }) => insets[0]);
  if (animated) expect(positions.some((left) => left > Math.min(...positions) + 1
    && left < Math.max(...positions) - 1)).toBe(true);
  for (const frame of frames) {
    expect(frame.retained).toBe(true);
    expect(frame.visible).toBe(true);
    expect(frame.focus).toBe(true);
    expect(frame.hit, JSON.stringify(frame)).toBe(true);
    expect(frame.paneEdge).toBe(true);
    expect(frame.separate).toBe(true);
    expect(frame.contained).toBe(true);
    expect(frame.content).toBe(true);
    expect(frame.console).toBe(frame.barLeft);
    expect(frame.scroll).toBe(true);
    expect(frame.sizes[0].width).toBeGreaterThanOrEqual(29.5);
    expect(frame.sizes[0].width).toBeLessThanOrEqual(44);
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
      const floating = await page.evaluate(() => {
        const view = document.querySelector("#view").getBoundingClientRect();
        const box = (selector) => document.querySelector(selector).getBoundingClientRect();
        const target = box(".tb-switch");
        const glyph = box(".tb-switch svg");
        return { glyphX: glyph.left - view.left, directoryGap: glyph.left - box("#dir-rail").right,
          targetWidth: target.width, targetHeight: target.height, toggleGap: target.left - box("#inbox-open").right,
          crumbGap: box(".tb-project").left - glyph.right };
      });
      console.log(`Floating inbox chevron ${width}px ${theme}: ${JSON.stringify(floating)}`);
      expect(floating.glyphX).toBe(60);
      expect(floating.directoryGap).toBe(20);
      expect(floating.targetWidth).toBe(44);
      expect(floating.targetHeight).toBe(44);
      expect(floating.toggleGap).toBeGreaterThanOrEqual(0);
      expect(floating.crumbGap).toBe(4);
      await page.locator(".tb-switch").focus();
      const pin = await samplePinTransition(page, false);
      const pinned = await toolbarGeometry(page);
      await captureLayout(page, `pinned-inbox-${width}-${theme}-pinned.png`);
      expect(await page.locator("#inbox-open").isVisible()).toBe(false);
      const target = await page.locator(".tb-switch").boundingBox();
      expect(target.width).toBe(29.5);
      expect(target.height).toBe(44);
      expect(target.x).toBeGreaterThanOrEqual((await page.locator("#toolbar").boundingBox()).x);
      for (const x of [target.x + 0.5, target.x + target.width / 2, target.x + target.width - 0.5]) {
        expect(await page.evaluate(({ x, y }) => document.querySelector(".tb-switch")
          .contains(document.elementFromPoint(x, y)), { x, y: target.y + target.height / 2 })).toBe(true);
      }
      const paneEdge = await page.evaluate(() => {
        const pane = document.querySelector("#inbox-rail");
        const rect = pane.getBoundingClientRect();
        window.__paneEdgeHit = false;
        pane.addEventListener("pointerdown", () => { window.__paneEdgeHit = true; }, { once: true });
        return { x: rect.right - 1, y: document.querySelector(".tb-switch").getBoundingClientRect().top + 22 };
      });
      await page.mouse.click(paneEdge.x, paneEdge.y);
      expect(await page.evaluate(() => window.__paneEdgeHit)).toBe(true);
      await page.locator(".tb-switch").focus();
      const unpin = await samplePinTransition(page, true);
      expectStableTransition(pin);
      expectStableTransition(unpin);
      expect(Math.abs(pinned.glyph - unpinned.inboxGlyph)).toBeLessThanOrEqual(1);
      const shift = unpinned.glyph - pinned.glyph;
      expect(unpinned.project - pinned.project).toBeCloseTo(shift, 1);
      expect(unpinned.workspaces - pinned.workspaces).toBeCloseTo(shift, 1);
      expect(await page.locator(".tb-switch").evaluate((button) => button.getBoundingClientRect().width)).toBe(44);
      expect(pinned.height).toBe(44);
      expect(await toolbarGeometry(page)).toEqual(unpinned);
      expect(await page.locator("#inbox-open").isVisible()).toBe(true);
      // Toolbar spacing must not move a full console.
      await page.evaluate(() => {
        const console = document.querySelector("#console-region");
        console.innerHTML = '<div class="console">Console</div>';
        console.dataset.size = "full";
      });
      expect(await page.evaluate(() => document.querySelector("#console-region").getBoundingClientRect().left))
        .toBe(await page.evaluate(() => document.querySelector("#toolbar").getBoundingClientRect().left));
    }, { width, height: 800 });
  }, 30_000);
}

for (const motion of ["no-preference", "reduce"]) {
  it(`retains nodes, console bounds, scroll and focus across reversal and resizing with ${motion} motion`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: motion });
      await mountNavigationBreadcrumbs(page, basePath, { kind: "project", inboxHeader: true });
      await page.evaluate(async () => {
        await window.__layoutModules.inbox.setInboxCollapsed(true, { animate: false, persist: false, reveal: false });
        const console = document.querySelector("#console-region");
        console.innerHTML = '<div class="console"><div class="console-head">Console</div><div id="console-scroll" style="overflow:auto"><button class="btn" id="console-focus" style="position:sticky;top:12px;margin:12px">Console focus</button><div style="height:2000px">Console content</div></div></div>';
        console.dataset.size = "full";
        document.querySelector("#console-scroll").scrollTop = 180;
        document.querySelector("#console-focus").focus({ preventScroll: true });
        window.__keptSwitcher = document.querySelector(".tb-switch");
      });
      await settle(page);
      expectStableTransition(await samplePinTransition(page, false), { animated: motion !== "reduce" });
      expectStableTransition(await samplePinTransition(page, true), { animated: motion !== "reduce" });
      expectStableTransition(await samplePinTransition(page, false, { reverseAfter: 80 }), { animated: motion !== "reduce" });
      expect(await page.evaluate(() => document.body.classList.contains("inbox-collapsed"))).toBe(true);
      await page.evaluate(() => window.__layoutModules.inbox.setInboxCollapsed(false, { animate: true, persist: false, reveal: false }));
      await page.waitForTimeout(60);
      await page.setViewportSize({ width: 900, height: 800 });
      await settle(page);
      expect(await page.locator("#view").evaluate((view) => {
        const style = getComputedStyle(view);
        return [style.marginLeft, style.paddingLeft];
      })).toEqual(["0px", "0px"]);
      await page.setViewportSize({ width: 1280, height: 800 });
      await settle(page);
      expect(await page.evaluate(() => document.querySelector(".tb-switch") === window.__keptSwitcher)).toBe(true);
      expect(await page.evaluate(() => document.activeElement?.id)).toBe("console-focus");
      expect(await page.locator("#console-scroll").evaluate((scroll) => scroll.scrollTop)).toBe(180);
      expect(await page.evaluate(() => document.querySelector("#console-region").getBoundingClientRect().left))
        .toBe(await page.evaluate(() => document.querySelector("#toolbar").getBoundingClientRect().left));
    }, { width: 1280, height: 800 });
  }, 30_000);
}

for (const width of [390, 900]) {
  it(`keeps the narrow inbox overlay trigger and unpinned spacing ${width}px`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountNavigationBreadcrumbs(page, basePath, { kind: "project", inboxHeader: true });
      await settle(page);
      const before = await toolbarGeometry(page);
      expect(before.glyph).toBe(60);
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
