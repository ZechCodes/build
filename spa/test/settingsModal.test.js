// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
const { App, go, markRoute, local, device, listeners } = vi.hoisted(() => ({
  App: { route: { name: "account", page: "settings" }, devices: [], viewDispose: null },
  go: vi.fn(), markRoute: vi.fn(), local: vi.fn(), device: vi.fn(), listeners: new Set(),
}));
vi.mock("../src/app.js", () => ({ App, go, markRoute }));
vi.mock("../src/views/settings.js", () => ({ renderSettings: local }));
vi.mock("../src/views/deviceSettings.js", () => ({ renderDeviceSettings: device }));
vi.mock("../src/core/deviceContexts.js", () => ({ onDeviceStateChanged: (listener) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
} }));
import { isSettingsRoute, renderSettingsModal } from "../src/views/settingsModal.js";
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '<div id="shell"><main id="root">Workspace content</main></div><div id="scrim"></div>';
  App.devices = [{ id: "a", name: "Laptop", status: "online" }, { id: "b", name: "Server", status: "offline" }];
  App.route = { name: "account", page: "settings" };
  local.mockImplementation(async ({ root }) => { root.innerHTML = '<button>Preference</button>'; });
  device.mockImplementation(async ({ root, deviceId }) => { root.textContent = deviceId; });
});
afterEach(() => { App.viewDispose?.(); App.viewDispose = null; });
describe("settings modal", () => {
  it("lists local settings and devices over the existing surface, returning on close", async () => {
    const returnRoute = { name: "workspace", deviceId: "a", workspaceId: "w1" };
    renderSettingsModal(returnRoute);
    await flush();
    expect(document.querySelector('[role="dialog"]').textContent).toContain("Local settings");
    expect(document.querySelectorAll('[data-settings-device]')).toHaveLength(2);
    expect(document.querySelector('#root').textContent).toBe("Workspace content");
    expect(document.querySelector('#shell').inert).toBe(true);
    document.querySelector('[data-settings-close]').click();
    expect(go).toHaveBeenCalledWith(returnRoute);
    App.viewDispose(); App.viewDispose = null;
    expect(document.querySelector('.settings-scrim')).toBeNull();
    expect(document.querySelector('#shell').inert).toBeFalsy();
    expect(listeners.size).toBe(0);
  });
  it("disposes a device panel when switching and isolates late local results", async () => {
    let finish;
    let localOptions;
    local.mockImplementation((options) => {
      localOptions = options;
      return new Promise((resolve) => { finish = resolve; });
    });
    const dispose = vi.fn();
    device.mockImplementation(async ({ root, deviceId, registerDispose }) => {
      root.textContent = `Device ${deviceId}`;
      registerDispose(dispose);
    });
    renderSettingsModal();
    document.querySelector('[data-settings-device="b"]').click();
    await flush();
    expect(localOptions.isCurrent()).toBe(false);
    expect(markRoute).toHaveBeenLastCalledWith({ name: "device", id: "b" });
    localOptions.root.textContent = "Late answer";
    finish(); await flush();
    expect(document.querySelector('.settings-content').textContent).toBe("Device b");
    document.querySelector('[data-local]').click();
    expect(dispose).toHaveBeenCalledOnce();
  });
  it("opens device deep links and lets nested sheets own Escape", () => {
    App.route = { name: "device", id: "b" };
    renderSettingsModal();
    expect(device.mock.calls[0][0].deviceId).toBe("b");
    document.querySelector('#scrim').classList.add("show");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(go).not.toHaveBeenCalled();
    document.querySelector('#scrim').classList.remove("show");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(go).toHaveBeenCalledWith({ name: "inbox" });
  });
  it("keeps keyboard focus in a nested sheet", () => {
    renderSettingsModal();
    const sheet = document.querySelector('#scrim');
    sheet.classList.add('show');
    sheet.innerHTML = '<button id="first">Cancel</button><button id="last">Choose</button>';
    for (const button of sheet.querySelectorAll('button')) button.getClientRects = () => [new DOMRect(0, 0, 20, 20)];
    sheet.querySelector('#last').focus();
    const tab = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true });
    document.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement.id).toBe('first');
    document.querySelector('[data-settings-close]').focus();
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }));
    expect(document.activeElement.id).toBe('first');
  });
  it("retains archive as a page", () => {
    expect(isSettingsRoute({ name: "account", page: "archive" })).toBe(false);
    expect(isSettingsRoute({ name: "account", page: "devices" })).toBe(true);
    expect(isSettingsRoute({ name: "device", id: "a" })).toBe(true);
  });
});
