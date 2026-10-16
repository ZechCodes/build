import { afterAll, beforeAll, expect, it } from "vitest";
import { openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { changeWindowControls, installWindowControls, mountWindowControls, windowControlsGeometry } from "./windowControlsSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [navigationAppShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

for (const width of [1280, 901, 900, 600]) for (const pinned of [true, false]) {
  it(`reserves native controls in the top-left header (${width}px, pinned=${pinned})`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await page.emulateMedia({ reducedMotion: "reduce" });
      await mountWindowControls(page, basePath, { pinned });
      const geometry = await windowControlsGeometry(page);
      const docked = pinned && width > 900;
      expect(geometry.toolbar.y).toBe(0);
      expect(geometry.toolbar.height).toBeGreaterThanOrEqual(44);
      expect(geometry.firstToolbar.height).toBe(44);
      if (docked) {
        expect(geometry.head.y).toBe(0);
        expect(geometry.head.height).toBe(44);
        expect(geometry.firstInbox.x).toBe(84);
        expect(geometry.firstToolbar.x).toBeLessThan(geometry.toolbar.x + 84);
        expect(await page.locator("#inbox-new-project span").evaluate((element) => {
          const range = document.createRange();
          range.selectNodeContents(element);
          return range.getClientRects().length;
        })).toBe(1);
      } else {
        expect(geometry.toggle.x).toBeGreaterThanOrEqual(84);
        expect(geometry.firstToolbar.x).toBeGreaterThanOrEqual(geometry.toggle.right);
        expect(geometry.firstInbox.x).toBeLessThan(84);
      }
      expect(geometry.scrollWidth).toBeLessThanOrEqual(width);
      await page.evaluate(async (pinned) => {
        await window.__layoutModules.inbox.setInboxCollapsed(pinned, { animate: false, persist: false, reveal: false });
      }, pinned);
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const toggled = await windowControlsGeometry(page);
      if (width > 900 && !pinned) expect(toggled.firstInbox.x).toBe(84);
      else expect(toggled.toggle.x).toBeGreaterThanOrEqual(84);
    }, { width, height: 800 });
  }, 30_000);
}

it("follows geometry changes, fullscreen restore and a pinned inbox crossing the overlay breakpoint", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountWindowControls(page, basePath);
    await changeWindowControls(page, { x: 104 });
    expect((await windowControlsGeometry(page)).firstInbox.x).toBe(104);
    await page.setViewportSize({ width: 600, height: 800 });
    expect((await windowControlsGeometry(page)).toggle.x).toBeGreaterThanOrEqual(104);
    await changeWindowControls(page, { visible: false, x: 0, height: 0 });
    const fullscreen = await windowControlsGeometry(page);
    expect(fullscreen.toolbar.y).toBe(10);
    expect(fullscreen.toolbar.drag).toBe("none");
    await changeWindowControls(page);
    expect((await windowControlsGeometry(page)).toolbar.y).toBe(0);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    expect((await windowControlsGeometry(page)).firstInbox.x).toBe(84);
  }, { width: 1280, height: 800 });
}, 30_000);

it("drags empty bars and keeps all controls, their children and portalled menus out of drag regions", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWindowControls(page, basePath, { pinned: false });
    const geometry = await windowControlsGeometry(page);
    expect(geometry.toolbar.drag).toBe("drag");
    expect(geometry.toggle.drag).toBe("no-drag");
    expect(await page.locator(".rail-head").evaluate((element) => getComputedStyle(element).webkitAppRegion)).toBe("drag");
    await page.locator('[data-select="project"]').click();
    await page.locator(".tb-filter").fill("Docs");
    expect(await page.locator(".tb-filter").inputValue()).toBe("Docs");
    const drags = await page.evaluate(() => [...document.querySelectorAll(
      '.toolbar :is(button, a, input, select, textarea), .toolbar :is(button, a) *, .inbox-head button, .inbox-head button *, .rail-head button, .rail-head button *, #inbox-open *, .tbmenu, .tbmenu *',
    )].map((element) => ({ tag: element.tagName, drag: getComputedStyle(element).webkitAppRegion })));
    expect(drags.length).toBeGreaterThan(12);
    expect(drags.filter(({ drag }) => drag !== "no-drag")).toEqual([]);
    await page.locator('[data-project="nav-device/docs"]').click();
    expect(await page.evaluate(() => window.__navigationHarness.navigations.at(-1).projectId)).toBe("docs");
  }, { width: 1280, height: 800 });
}, 30_000);

it("leaves browser and standalone PWA geometry and drag behavior unchanged with an inactive overlay", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountWindowControls(page, basePath, { install: false, pinned: false });
    const before = await windowControlsGeometry(page);
    await changeWindowControls(page, { visible: false });
    await installWindowControls(page, basePath);
    expect(await windowControlsGeometry(page)).toEqual(before);
    expect(before.toolbar.drag).toBe("none");
    expect(before.toolbar.y).toBe(10);
  }, { width: 1280, height: 800 });
}, 30_000);

it.each([true, false])("clears the version banner when it owns the top-left (pinned=%s)", async (pinned) => {
  await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountWindowControls(page, basePath, { pinned });
    await page.locator("#verbar").evaluate((element) => { element.hidden = false; });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const banner = await page.evaluate(() => {
      const element = document.querySelector("#verbar");
      return { height: element.getBoundingClientRect().height,
        textLeft: element.querySelector("span").getBoundingClientRect().left,
        drag: getComputedStyle(element).webkitAppRegion,
        buttonDrag: getComputedStyle(element.querySelector("button")).webkitAppRegion };
    });
    expect(banner.height).toBeGreaterThanOrEqual(44);
    expect(banner.textLeft).toBeGreaterThanOrEqual(84);
    expect(banner.drag).toBe("drag");
    expect(banner.buttonDrag).toBe("no-drag");
    const geometry = await windowControlsGeometry(page);
    expect(geometry.firstInbox.x).toBeLessThan(84);
    if (!pinned) expect(geometry.toggle.x).toBeLessThan(84);
  }, { width: 1280, height: 800 });
}, 30_000);

it("keeps the floating inbox caret below its displaced toggle", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mountWindowControls(page, basePath, { pinned: false });
    await page.locator("#inbox-open").click();
    const centers = await page.evaluate(() => {
      const rail = document.querySelector("#inbox-rail");
      const toggle = document.querySelector("#inbox-open").getBoundingClientRect();
      const caret = getComputedStyle(rail, "::before");
      return { toggle: toggle.x + toggle.width / 2,
        caret: rail.getBoundingClientRect().left + parseFloat(getComputedStyle(rail).borderLeftWidth) + parseFloat(caret.left) };
    });
    expect(centers.caret).toBeCloseTo(centers.toggle, 0);
  }, { width: 1280, height: 800 });
}, 30_000);

it("keeps a drag surface on gated screens without overlapping their content", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWindowControls(page, basePath);
    await page.locator("body").evaluate((element) => { element.classList.add("gated"); });
    const strip = await page.locator("#shell").evaluate((element) => {
      const style = getComputedStyle(element, "::before");
      return { drag: style.webkitAppRegion, height: style.height, top: style.top };
    });
    expect(strip).toEqual({ drag: "drag", height: "44px", top: "0px" });
  }, { width: 1280, height: 800 });
}, 30_000);

it("keeps header controls clear of native buttons throughout pin and unpin animations", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWindowControls(page, basePath);
    const overlaps = await page.evaluate(async () => {
      const hits = [];
      for (const { collapsed, reverseAfter } of [{ collapsed: true }, { collapsed: false }, { collapsed: true, reverseAfter: 60 }]) {
        await window.__layoutModules.inbox.setInboxCollapsed(collapsed, { persist: false, reveal: false });
        if (reverseAfter) setTimeout(() => {
          void window.__layoutModules.inbox.setInboxCollapsed(!collapsed, { persist: false, reveal: false });
        }, reverseAfter);
        const until = performance.now() + 500;
        do {
          await new Promise(requestAnimationFrame);
          for (const element of document.querySelectorAll('.inbox-head button, .toolbar button, .toolbar a, #inbox-open')) {
            const rect = element.getBoundingClientRect();
            const style = getComputedStyle(element);
            if (rect.width && rect.height && style.visibility === "visible" && rect.left < 84 && rect.right > 12 && rect.top < 28 && rect.bottom > 16) {
              hits.push(element.id || element.className);
            }
          }
        } while (performance.now() < until);
      }
      return [...new Set(hits)];
    });
    expect(overlaps).toEqual([]);
  }, { width: 1280, height: 800 });
}, 30_000);
