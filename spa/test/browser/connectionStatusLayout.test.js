import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const connectionFixture = {
  name: "ring-connection-fixture", enforce: "pre",
  resolveId(source, importer) {
    if (!importer?.endsWith("/src/connectionStatus.js")) return null;
    if (source === "./connection.js") return "\0ring-recovery";
    if (source === "./core/deviceContexts.js") return "\0ring-contexts";
    return null;
  },
  load(id) {
    if (id === "\0ring-recovery") return `
      export const deviceRecoverySnapshot = () => [];
      export const onDeviceRecoveryChanged = () => () => {};`;
    if (id === "\0ring-contexts") return `
      export const liveContexts = () => [window.ringContext];
      export const onDeviceStateChanged = (paint) => { window.paintRing = paint; return () => {}; };`;
    return null;
  },
};

let session;
beforeAll(async () => {
  session = await openLayoutSession({ plugins: [connectionFixture] });
}, 30_000);
afterAll(async () => session?.close());

async function openRing(page, basePath, terminalTransport, styles = "") {
  await mountLayout(page, '<div id="shell"><section id="view"><div id="connection-status" hidden></div></section></div>', {
    basePath, styles,
  });
  await loadBrowserModules(page, {
    app: "src/appState.js", ring: "src/connectionStatus.js",
  }, basePath);
  await page.evaluate((terminalTransport) => {
    const { app, ring } = window.__layoutModules;
    document.body.classList.add("inbox-collapsed");
    app.App.devices = [{ id: "dev", name: "Development laptop" }];
    window.ringContext = { deviceId: "dev", session: { transport: "http", terminalTransport } };
    ring.mountConnectionStatus();
  }, terminalTransport);
  await page.locator(".connection-status").click();
  await page.getByRole("menu", { name: "Devices" }).waitFor({ state: "visible" });
}

async function expectVisibleLabel(page) {
  const geometry = await page.locator(".connection-menu .md").evaluate((label) => {
    const range = document.createRange();
    range.selectNodeContents(label);
    const bounds = (box) => [box.left, box.top, box.right, box.bottom];
    return {
      menu: bounds(label.closest(".connection-menu").getBoundingClientRect()),
      viewport: [0, 0, document.documentElement.clientWidth, document.documentElement.clientHeight],
      text: [...range.getClientRects()].map(bounds),
    };
  });
  const inside = ([left, top, right, bottom], [minX, minY, maxX, maxY]) =>
    left >= minX && top >= minY && right <= maxX && bottom <= maxY;
  const detail = JSON.stringify(geometry);
  expect(inside(geometry.menu, geometry.viewport), detail).toBe(true);
  expect(geometry.text.length, detail).toBeGreaterThan(0);
  for (const line of geometry.text) expect(inside(line, geometry.menu), detail).toBe(true);
}

it.each([
  { width: 320, corner: "bottom-right", styles: "" },
  { width: 1280, corner: "bottom-right", styles: "" },
  { width: 320, corner: "top-left", styles: "#connection-status { inset:0 auto auto 0; }" },
])("keeps the full HTTPS fallback label visible at $width px in the $corner corner", async ({ width, corner, styles }) => {
  await session.withPage(async ({ page, basePath }) => {
    await openRing(page, basePath, "webrtc", styles);
    const ring = await page.locator(".connection-status").boundingBox();
    if (corner === "bottom-right") {
      expect(width - ring.x - ring.width).toBeLessThan(32);
      expect(480 - ring.y - ring.height).toBeLessThan(32);
    } else {
      expect(ring.x).toBeLessThan(32);
      expect(ring.y).toBeLessThan(32);
    }
    await captureLayout(page, `connection-ring-${width}-${corner}.png`);
    await expectVisibleLabel(page);
  }, { width, height: 480 });
}, 30_000);

it("updates the visible label when the HTTPS Opening selects a different terminal carrier", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await openRing(page, basePath, "wss");
    await captureLayout(page, "connection-ring-https.png");
    const label = page.locator(".connection-menu .md");
    expect(await label.textContent()).toBe("Connected HTTPS");
    await expectVisibleLabel(page);
    await page.evaluate(() => {
      window.ringContext.session = { transport: "http", terminalTransport: "webrtc" };
      window.paintRing();
    });
    expect(await label.textContent()).toBe("Connected HTTPS · Terminal over WebRTC");
    await expectVisibleLabel(page);
    await page.setViewportSize({ width: 1280, height: 480 });
    await expect.poll(async () => {
      const ring = await page.locator(".connection-status").boundingBox();
      const menu = await page.getByRole("menu", { name: "Devices" }).boundingBox();
      return Math.abs(ring.x + ring.width - menu.x - menu.width);
    }).toBeLessThan(2);
    await expectVisibleLabel(page);
  }, { width: 320, height: 480 });
}, 30_000);
