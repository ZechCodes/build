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
      export const onDeviceStateChanged = () => () => {};`;
    return null;
  },
};

let session;
beforeAll(async () => {
  session = await openLayoutSession({ plugins: [connectionFixture] });
}, 30_000);
afterAll(async () => session?.close());

async function openRing(page, basePath, { mode, entitled = false, styles = "" }) {
  await mountLayout(page, '<div id="shell"><section id="view"><div id="connection-status" hidden></div></section></div>', {
    basePath, styles,
  });
  await loadBrowserModules(page, {
    app: "src/appState.js", ring: "src/connectionStatus.js", http: "src/core/httpConnections.js",
  }, basePath);
  await page.evaluate(async ({ mode, entitled }) => {
    const { app, ring, http } = window.__layoutModules;
    const accountId = "10000000-0000-4000-8000-000000000061";
    const origin = "https://127.0.0.1:8443";
    document.head.insertAdjacentHTML("beforeend", `<meta name="build-account-id" content="${accountId}">`);
    document.body.classList.add("inbox-collapsed");
    app.App.deviceTransportPolicy = { accountId, policyAware: true, tunnelAccess: entitled };
    app.App.devices = [{ id: "dev", name: "Development laptop", transport: {
      source: entitled ? "hosted" : "personal", rtc_allowed: !entitled, reason: null, origin,
      assignment_revision: entitled ? 1 : null,
    } }];
    await http.restoreHttpConnections();
    if (!entitled) await http.selectHttpConnection({ deviceId: "dev", origin });
    await http.rememberHttpTerminalMode("dev", mode, () => true);
    // Deliberately disagree with the committed carrier. The ring's connection
    // state supplies the HTTPS path, but the terminal label follows the cache.
    const terminalTransport = entitled ? "webrtc" : "wss";
    window.ringContext = { deviceId: "dev", session: { transport: "http", terminalTransport } };
    ring.mountConnectionStatus();
  }, { mode, entitled });
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
    await openRing(page, basePath, { mode: "webrtc", styles });
    expect(await page.locator(".connection-menu .md").textContent()).toBe("Connected HTTPS · Terminal over WebRTC");
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

it("updates the manual HTTPS label when terminal compatibility commits a WebRTC fallback", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await openRing(page, basePath, { mode: "wss" });
    await captureLayout(page, "connection-ring-https.png");
    const label = page.locator(".connection-menu .md");
    expect(await label.textContent()).toBe("Connected HTTPS");
    await expectVisibleLabel(page);
    await page.evaluate(() => window.__layoutModules.http.rememberHttpTerminalMode("dev", "webrtc", () => true));
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

it("never labels entitled HTTPS as a WebRTC fallback when terminal compatibility requires an update", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await openRing(page, basePath, { mode: "wss", entitled: true });
    const label = page.locator(".connection-menu .md");
    expect(await label.textContent()).toBe("Connected HTTPS");

    const cachedMode = await page.evaluate(async () => {
      const { http } = window.__layoutModules;
      await http.rememberHttpTerminalMode("dev", "update_required", () => true);
      return http.cachedTerminalTransport("dev");
    });
    expect(cachedMode).toBe("update_required");
    expect(await label.textContent()).toBe("Connected HTTPS");
    await expectVisibleLabel(page);
  }, { width: 320, height: 480 });
}, 30_000);
