import { loadBrowserModules } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs } from "./navigationBreadcrumbSeed.mjs";

// A standard WCO geometry signal, never an Electron preload or real account.
// Native traffic lights cannot be painted by headless Chromium: this optional
// review probe shows an illustrative footprint over the production shell.
// The 84px reservation and 12px circles are synthetic, not measured AppKit sizes.
export async function mountWindowControls(page, basePath, { install = true, pinned = true, lights = false, theme = "dark", visible = true } = {}) {
  await page.evaluate(() => { window.__stopWindowControls?.(); });
  await page.evaluate(({ visible }) => {
    const overlay = new EventTarget();
    overlay.visible = visible;
    overlay.getTitlebarAreaRect = () => new DOMRect(84, 0, innerWidth - 84, 44);
    Object.defineProperty(navigator, "windowControlsOverlay", { configurable: true, value: overlay });
  }, { visible });
  await mountNavigationBreadcrumbs(page, basePath, { theme, kind: "project", inboxHeader: true });
  await installWindowControls(page, basePath, { active: install });
  await page.evaluate(async ({ pinned, lights }) => {
    await window.__layoutModules.inbox.setInboxCollapsed(!pinned, { animate: false, persist: false, reveal: false });
    if (lights) {
      const probe = document.createElement("div");
      probe.id = "simulated-native-controls";
      probe.setAttribute("aria-hidden", "true");
      probe.style.cssText = "position:fixed;z-index:10000;left:12px;top:16px;display:flex;gap:8px;pointer-events:none";
      for (const color of ["#ff5f57", "#febc2e", "#28c840"]) {
        const light = document.createElement("i");
        light.style.cssText = `width:12px;height:12px;border-radius:50%;background:${color};box-shadow:inset 0 0 0 1px #0002`;
        probe.append(light);
      }
      document.body.append(probe);
    }
  }, { pinned, lights });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

export async function installWindowControls(page, basePath, { active = true } = {}) {
  await page.evaluate(() => { window.__navigationModules = window.__layoutModules; });
  await loadBrowserModules(page, { chrome: "src/core/windowControlsOverlay.js", rail: "src/core/agentRail.js" }, basePath);
  await page.evaluate((active) => {
    if (active) window.__stopWindowControls = window.__layoutModules.chrome.installWindowControlsOverlay();
    document.querySelector(".rail-head").outerHTML = window.__layoutModules.rail.panelHeadHtml("Desktop chrome", "chat", {
      removable: true, hasTerminal: true, pinned: true, watch: { watching: true },
    });
    window.__layoutModules = window.__navigationModules;
  }, active);
}

export async function changeWindowControls(page, { visible = true, x = 84, height = 44 } = {}) {
  await page.evaluate(({ visible, x, height }) => {
    const overlay = navigator.windowControlsOverlay;
    overlay.visible = visible;
    overlay.getTitlebarAreaRect = () => new DOMRect(x, 0, innerWidth - x, height);
    overlay.dispatchEvent(new Event("geometrychange"));
  }, { visible, x, height });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

export async function windowControlsGeometry(page) {
  return page.evaluate(() => {
    const box = (selector) => {
      const node = document.querySelector(selector);
      const { x, y, right, bottom, width, height } = node.getBoundingClientRect();
      return { x, y, right, bottom, width, height, drag: getComputedStyle(node).webkitAppRegion };
    };
    return { toolbar: box(".toolbar"), head: box(".inbox-head"), toggle: box("#inbox-open"),
      firstInbox: box(".inbox-views"), firstToolbar: box(".tb-switch"),
      scrollWidth: document.documentElement.scrollWidth };
  });
}
