import { readFileSync } from "node:fs";
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, openLayoutSession } from "./layoutHarness.mjs";
import { mountNavigationBreadcrumbs, navigationAppShim } from "./navigationBreadcrumbSeed.mjs";
import { changeWindowControls, installWindowControls } from "./windowControlsSeed.mjs";

const connectionFixture = {
  name: "channel-connection-fixture", enforce: "pre",
  config: () => ({ define: { "import.meta.env.VITE_BUILD_CHANNEL": JSON.stringify("dev") } }),
  resolveId(source, importer) {
    if (!importer?.endsWith("/src/connectionStatus.js")) return null;
    if (source === "./connection.js") return "\0channel-recovery";
    if (source === "./core/deviceContexts.js") return "\0channel-contexts";
    return null;
  },
  load(id) {
    if (id === "\0channel-recovery") return `
      export const deviceRecoverySnapshot = () => window.__channelState.recoveries;
      export const onDeviceRecoveryChanged = listener => { window.__channelPaint = listener; return () => {}; };`;
    if (id === "\0channel-contexts") return `
      export const liveContexts = () => window.__channelState.live.map(deviceId => ({ deviceId }));
      export const onDeviceStateChanged = () => () => {};`;
    return null;
  },
};
let session;
beforeAll(async () => {
  session = await openLayoutSession({ plugins: [navigationAppShim, connectionFixture] });
}, 30_000);
afterAll(async () => session?.close());

const overlaps = (a, b) => Math.min(a.x + a.width, b.x + b.width) > Math.max(a.x, b.x)
  && Math.min(a.y + a.height, b.y + b.height) > Math.max(a.y, b.y);


async function assertBubbleGeometry(page, width) {
  const geometry = await page.evaluate(() => {
    const box = (node) => node.getBoundingClientRect().toJSON();
    const controls = [...document.querySelectorAll("button, input, textarea, .connection-centre")]
      .filter((node) => node.checkVisibility() && !node.matches(".connection-status"))
      .map((node) => ({ name: node.className, box: box(node) }));
    return { ring: box(document.querySelector(".connection-status")),
      bubble: box(document.querySelector(".connection-build-channel")), controls };
  });
  expect(overlaps(geometry.ring, geometry.bubble), "overlaps the ring corner").toBe(true);
  expect(geometry.bubble.x + geometry.bubble.width).toBeLessThanOrEqual(width);
  expect(geometry.bubble.y).toBeGreaterThanOrEqual(0);
  for (const control of geometry.controls) expect(overlaps(geometry.bubble, control.box), control.name).toBe(false);
}

for (const width of [375, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`anchors the Dev bubble without moving controls at ${width}px in ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await page.emulateMedia({ reducedMotion: "reduce" });
        await mountNavigationBreadcrumbs(page, basePath, { inboxHeader: true, theme });
        await page.evaluate(() => {
          window.__channelState = { live: ["nav-device"], recoveries: [] };
          window.__shellModules = window.__layoutModules;
          Object.defineProperty(navigator, "windowControlsOverlay", { configurable: true, value: Object.assign(new EventTarget(), {
            visible: true, getTitlebarAreaRect: () => new DOMRect(84, 0, innerWidth - 84, 44),
          }) });
        });
        await installWindowControls(page, basePath);
        await changeWindowControls(page);
        await loadBrowserModules(page, { status: "src/connectionStatus.js", channel: "src/core/buildChannel.js" }, basePath);
        await page.evaluate(() => {
          window.__layoutModules.channel.initBuildChannel();
          window.__layoutModules.status.mountConnectionStatus();
          window.__badge = document.querySelector(".connection-build-channel");
        });
        expect(await page.locator("#build-channel").count()).toBe(0);
        const badge = page.locator(".connection-build-channel");
        expect(await badge.isVisible()).toBe(true);
        expect(await badge.textContent()).toBe("D");
        for (const overlay of [true, false]) {
          await changeWindowControls(page, { visible: overlay });
          for (const collapsed of [false, true]) {
            await page.evaluate(async (collapsed) => {
              await window.__shellModules.inbox.setInboxCollapsed(collapsed, { animate: false, persist: false, reveal: false });
            }, collapsed);
            for (const state of ["connected", "attempting", "waiting", "offline"]) {
              await page.evaluate((state) => {
                window.__channelState.live = state === "connected" ? ["nav-device"] : [];
                window.__channelState.recoveries = ["attempting", "waiting"].includes(state)
                  ? [{ deviceId: "nav-device", status: state, failedAttempts: 1, nextAttemptAt: Date.now() + 3000 }] : [];
                window.__channelPaint();
              }, state);
              expect(await badge.isVisible(), state).toBe(true);
              expect(await badge.evaluate((node) => node === window.__badge)).toBe(true);
              expect(await page.locator(".connection-status").getAttribute("aria-description")).toBe("Dev build");
              await assertBubbleGeometry(page, width);
            }
          }
        }
        await page.locator(".connection-status").click();
        const menu = page.locator(".connection-menu");
        await menu.waitFor({ state: "visible" });
        expect(overlaps(await badge.boundingBox(), await menu.boundingBox())).toBe(false);
        await page.keyboard.press("Escape");
        await menu.waitFor({ state: "hidden" });
        expect(await page.locator(".connection-status").evaluate((node) => node === document.activeElement)).toBe(true);
        // Channel decoration is out of flow: removing it cannot change controls.
        const boxes = () => page.locator("#global-controls, .toolbar, .inbox-head, .connection-status").evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().toJSON()));
        const before = await boxes();
        await badge.evaluate((node) => { node.style.display = "none"; });
        expect(await boxes()).toEqual(before);
        await badge.evaluate((node) => { node.style.display = ""; });
        await page.evaluate(() => {
          window.__channelState = { live: ["nav-device"], recoveries: [] };
          window.__channelPaint();
        });
        await captureLayout(page, `build-channel-${width}-${theme}.png`);
      }, { width, height: 820 });
    }, 60_000);
  }
}

it("removes the standalone channel layout workarounds", () => {
  const css = readFileSync(new URL("../../src/styles.css", import.meta.url), "utf8");
  expect(css).not.toContain("#build-channel");
  expect(css).not.toContain("--build-channel-inset");
});
