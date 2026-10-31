import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

// The real overlays a terminal pane carries — the touch key bar, the
// reconnecting chip and the saved-transport label — mounted by their own
// modules. Only the saved preference and the socket's status are stand-ins.
const FAKES = {
  "/src/core/httpConnections.js": `
    export const usesHttp = (deviceId) => deviceId === "dev-http";
    export const onHttpPreferencesChanged = () => () => {};`,
  "/src/terminal/manager.js": `
    export const subscribeTerminalStatus = (fn) => { fn(window.__status || "connected"); return () => {}; };
    export const terminalManager = () => ({});`,
};
const labelFixture = {
  name: "terminal-transport-label-fixture", enforce: "pre",
  async resolveId(source, importer, options) {
    if (!importer || importer.startsWith("\0")) return null;
    const resolved = await this.resolve(source, importer, { ...options, skipSelf: true });
    const fake = resolved && Object.keys(FAKES).find((path) => resolved.id.endsWith(path));
    return fake ? `\0label-fake:${fake}` : null;
  },
  load(id) {
    return id.startsWith("\0label-fake:") ? FAKES[id.slice("\0label-fake:".length)] : null;
  },
};

let session;
beforeAll(async () => {
  session = await openLayoutSession({ plugins: [labelFixture] });
}, 30_000);
afterAll(async () => session?.close());

const WIDTHS = [320, 390, 1280];
const THEMES = ["light", "dark"];

/** A pane as pane.js leaves it (screen, then the key bar on touch), with the
 *  connection overlay and the label attached after, as their callers do. */
async function mountPane(page, basePath, { width, theme, keys, status }) {
  await page.setViewportSize({ width, height: 420 });
  await mountLayout(page, '<div class="termpane" id="pane"><div class="term-screen"></div></div>', {
    basePath, styles: "body { margin:0; } #pane { width:100vw; height:320px; }",
  });
  await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
  await loadBrowserModules(page, {
    tabs: "src/core/surfaceTabs.js", keyBar: "src/terminal/keyBar.js", touchKeys: "src/terminal/touchKeys.js",
  }, basePath);
  return page.evaluate(({ keys, status }) => {
    const { tabs, keyBar, touchKeys } = window.__layoutModules;
    const pane = document.getElementById("pane");
    window.__status = status;
    if (keys) keyBar.mountKeyBar(pane, { send: () => {}, sticky: touchKeys.createStickyModifiers() });
    tabs.attachConnectionOverlay(pane);
    tabs.attachTransportLabel(pane, "dev-http");
    const box = (node) => (node && !node.hidden ? node.getBoundingClientRect().toJSON() : null);
    const label = pane.querySelector(".term-transport");
    // The label never takes pointer events, so hit-test it as if it did: what
    // answers at its centre is whatever is painted over it.
    label.style.pointerEvents = "auto";
    const { x, y, width, height } = label.getBoundingClientRect();
    const hit = document.elementFromPoint(x + width / 2, y + height / 2);
    label.style.pointerEvents = "";
    return {
      pane: box(pane), screen: box(pane.querySelector(".term-screen")), keys: box(pane.querySelector(".termkeys")),
      chip: box(pane.querySelector(".term-conn")), label: box(label),
      hit: hit === label ? "label" : hit?.className || String(hit),
    };
  }, { keys, status });
}

const overlaps = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
const inside = (inner, outer) => inner.left >= outer.left && inner.right <= outer.right
  && inner.top >= outer.top && inner.bottom <= outer.bottom;

it("shows the transport label uncovered, clear of the key bar and the reconnecting chip, at every width and theme", async () => {
  await session.withPage(async ({ page, basePath }) => {
    for (const width of WIDTHS) {
      for (const theme of THEMES) {
        for (const keys of [true, false]) {
          const where = `${width}px ${theme} ${keys ? "touch keys" : "no keys"}`;
          const shape = await mountPane(page, basePath, { width, theme, keys, status: "disconnected" });
          const detail = `${where}: ${JSON.stringify(shape)}`;
          expect(shape.chip, where).not.toBeNull();
          expect(shape.hit, detail).toBe("label");
          expect(overlaps(shape.chip, shape.label), detail).toBe(false);
          expect(inside(shape.label, shape.screen), detail).toBe(true);
          if (keys) expect(overlaps(shape.keys, shape.label), detail).toBe(false);
          expect(shape.screen.right - shape.label.right, where).toBeCloseTo(12, 0);
          await captureLayout(page, `terminal-transport-${width}-${theme}-${keys ? "keys" : "nokeys"}.png`);
        }
      }
    }
  });
}, 120_000);

it("puts the label in the same place whether or not the chip shows", async () => {
  await session.withPage(async ({ page, basePath }) => {
    for (const width of WIDTHS) {
      const quiet = await mountPane(page, basePath, { width, theme: "light", keys: true, status: "connected" });
      const offline = await mountPane(page, basePath, { width, theme: "light", keys: true, status: "disconnected" });
      expect(quiet.chip, `${width}px`).toBeNull();
      expect(quiet.screen.bottom - quiet.label.bottom, `${width}px`).toBeCloseTo(8, 0);
      expect(offline.label, `${width}px`).toEqual(quiet.label);
    }
  });
}, 120_000);
