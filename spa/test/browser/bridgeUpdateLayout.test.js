import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const UPDATE = {
  running_version: "0.2.0", platform: "linux-x86_64", development_build: false,
  latest_release: { version: "0.3.0", tag: "bridge-v0.3.0", published_at: "2026-09-23T12:00:00Z" },
  last_checked_at: "2026-09-23T12:00:00Z", state: "available", last_error: null,
  update_available: true, can_install: true,
};

for (const { label, width, height, state } of [
  { label: "desktop", width: 1280, height: 820, state: "available" },
  { label: "mobile", width: 390, height: 760, state: "scheduled_when_idle" },
]) {
  it(`fits the bridge update settings on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountLayout(page, `<div class="settings-scrim"><section class="settings-modal" role="dialog" aria-label="Settings">
        <header><h2>Settings</h2><button class="btn" data-settings-close aria-label="Close settings">✕</button></header>
        <nav class="settings-sidebar" aria-label="Settings sections">
          <button class="btn" data-local>Local settings</button><h3>Devices</h3>
          <button class="btn has-bridge-update" data-settings-device="laptop" aria-current="page">Laptop
            <small>online<span class="bridge-update-sidebar-note">Update available</span></small>
            <span class="bridge-update-dot" aria-hidden="true"></span></button>
        </nav>
        <div class="settings-content"><div class="device-settings">
          <div class="board-head"><div><h1>Laptop settings</h1><p>The projects this machine holds, and how agents run on it.</p></div></div>
          <div id="device-updates-panel"></div>
        </div></div>
      </section></div>`, { basePath });
      await loadBrowserModules(page, { panel: "src/core/bridgeUpdatePanel.js" }, basePath);
      await page.evaluate((status) => {
        const host = document.querySelector("#device-updates-panel");
        window.__layoutModules.panel.mountBridgeUpdatePanel(host, {
          deviceId: "laptop", callRpc: async () => status,
        });
      }, { ...UPDATE, state });
      await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent.includes("0.3.0") ||
        document.querySelector(".bridge-update-state")?.textContent.includes("Queued"));
      const bounds = await page.evaluate(() => {
        const panel = document.querySelector(".bridge-update-panel").getBoundingClientRect();
        const actions = document.querySelector(".bridge-update-actions").getBoundingClientRect();
        return { panelRight: panel.right, actionsRight: actions.right, viewport: innerWidth };
      });
      expect(bounds.panelRight).toBeLessThanOrEqual(bounds.viewport);
      expect(bounds.actionsRight).toBeLessThanOrEqual(bounds.viewport);
      expect(await page.locator("[data-bridge-install-now]").count()).toBe(1);
      expect(await page.locator("[data-bridge-install-idle]").count()).toBe(1);
      await captureLayout(page, `${label === "desktop" ? "available-desktop" : "queued-mobile"}.png`);
    }, { width, height });
  }, 30_000);
}

async function mountDevelopmentPanel(page, basePath, status) {
  await mountLayout(page, '<div class="device-settings"><div id="device-updates-panel"></div></div>', { basePath });
  await loadBrowserModules(page, { panel: "src/core/bridgeUpdatePanel.js" }, basePath);
  await page.evaluate((answer) => {
    window.__layoutModules.panel.mountBridgeUpdatePanel(document.querySelector("#device-updates-panel"), {
      deviceId: "dev-box", callRpc: async () => answer,
    });
  }, status);
  await page.waitForFunction(() => document.querySelector(".bridge-update-state")?.textContent.includes("0.3.0"));
}

const DEVELOPMENT = { ...UPDATE, development_build: true, can_install: false };

it("draws a development build's disabled Install without the fill that says press me", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountDevelopmentPanel(page, basePath, { ...DEVELOPMENT, can_replace_development_build: false });
    const looks = await page.evaluate(() => {
      const style = (selector) => getComputedStyle(document.querySelector(selector));
      const command = document.querySelector(".bridge-update-command").getBoundingClientRect();
      return {
        now: style("[data-bridge-install-now]").backgroundColor,
        check: style("[data-bridge-check]").backgroundColor,
        disabled: document.querySelector("[data-bridge-install-now]").disabled,
        commandRight: command.right, viewport: innerWidth,
      };
    });
    expect(looks.disabled).toBe(true);
    expect(looks.now).toBe(looks.check);
    expect(looks.commandRight).toBeLessThanOrEqual(looks.viewport);
    await captureLayout(page, "development-cannot-replace-mobile.png");
  }, { width: 390, height: 760 });
}, 30_000);

it("shows the replacement warning on a development build the app can replace", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountDevelopmentPanel(page, basePath, { ...DEVELOPMENT, can_replace_development_build: true, running_from_cargo_target: true });
    await page.locator("[data-bridge-install-now]").click();
    const warning = await page.evaluate(() => {
      const box = document.querySelector(".bridge-update-confirm").getBoundingClientRect();
      return { right: box.right, height: box.height, viewport: innerWidth };
    });
    expect(warning.height).toBeGreaterThan(0);
    expect(warning.right).toBeLessThanOrEqual(warning.viewport);
    await captureLayout(page, "development-replace-warning-mobile.png");
  }, { width: 390, height: 760 });
}, 30_000);

it("renders the complete device settings view with a connected fixture bridge", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(String(error)));
    await mountLayout(page, '<div id="shell"><main id="root"></main></div><div id="scrim"></div>', { basePath });
    await loadBrowserModules(page, { app: "src/app.js" }, basePath);
    await page.evaluate(() => { delete window.__layoutModules; delete window.__layoutModuleError; });
    await loadBrowserModules(page, {
      app: "src/app.js", contexts: "src/core/deviceContexts.js",
      cache: "src/core/localCache.js", modal: "src/views/settingsModal.js",
      events: "src/core/changeEvents.js",
    }, basePath);
    await page.evaluate(async (update) => {
      const { app, contexts, cache, modal } = window.__layoutModules;
      const device = { id: "laptop", name: "Laptop", status: "online", fingerprint: "fixture-key" };
      app.App.devices = [device];
      app.App.route = { name: "device", id: "laptop" };
      await cache.writeCached(cache.DEVICES_ADDRESS, [device]);
      const settings = { projects_dir: "/home/ada/Projects", default_harness: "claude", agent_modes: {}, isolation: "worktree", isolation_available: { rift: false, reason: "Fixture" } };
      const session = {
        deviceId: "laptop", close() {}, peer() {}, onCarrier() {}, onPush() {},
        call: async (method) => {
          if (method === "bridge.update_status") return update;
          if (method === "project.list") return { projects: [] };
          if (method === "models.list") return { default_provider: "claude", providers: [] };
          return settings;
        },
      };
      contexts.adoptDeviceSession(session);
      window.__closeSettings = modal.renderSettingsModal();
    }, UPDATE);
    try {
      await page.waitForSelector(".bridge-update-state", { timeout: 5000 });
    } catch (error) {
      throw new Error(`${error.message}; page errors: ${errors.join(" | ")}; content: ${(await page.locator(".settings-content").textContent()).slice(0, 1200)}`);
    }
    expect(await page.locator(".settings-content #device-settings-title").textContent()).toBe("Laptop settings");
    await page.locator("#device-updates-panel").scrollIntoViewIfNeeded();
    await captureLayout(page, "full-settings-available.png");
    await page.evaluate((update) => window.__layoutModules.events.dispatchChangeEvent({
      type: "bridge.update_status", ...update, state: "failed", last_error: "Health check failed; previous bridge restored.",
    }, "laptop"), UPDATE);
    await page.waitForSelector(".bridge-update-error");
    await page.locator("#device-updates-panel").scrollIntoViewIfNeeded();
    await captureLayout(page, "full-settings-failed.png");
  }, { width: 1280, height: 820 });
}, 60_000);

it("shows the update bubble on the inbox cog and device picker", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, '<div class="inbox-foot" style="position:absolute;left:44px;bottom:32px;width:300px;background:var(--panel);border-radius:10px"><div id="devpick"></div><button id="nav-account" class="inbox-account" aria-label="Settings"></button></div>', { basePath });
    await loadBrowserModules(page, { app: "src/app.js" }, basePath);
    await page.evaluate(() => { delete window.__layoutModules; delete window.__layoutModuleError; });
    await loadBrowserModules(page, {
      app: "src/app.js", devices: "src/devices.js", shell: "src/core/inboxShell.js",
      updates: "src/core/bridgeUpdates.js", icons: "src/core/icons.js",
    }, basePath);
    await page.evaluate(async (update) => {
      const { app, devices, shell, updates, icons } = window.__layoutModules;
      app.App.devices = [{ id: "laptop", name: "Laptop", status: "online" }];
      app.App.gated = false;
      document.querySelector("#nav-account").innerHTML = icons.ICON_SETTINGS;
      await updates.rememberBridgeUpdateStatus("laptop", update);
      devices.paintDevicePicker();
      document.querySelector(".device-picker-menu").hidden = false;
      shell.paintBridgeUpdateMark();
    }, UPDATE);
    await page.waitForSelector("#nav-account .bridge-update-dot");
    expect(await page.locator('[data-settings-device="laptop"] .bridge-update-dot').count()).toBe(1);
    await page.evaluate(() => { document.querySelector(".device-picker-menu").hidden = false; });
    await captureLayout(page, "inbox-cog-badge.png");
  }, { width: 420, height: 340 });
}, 30_000);
