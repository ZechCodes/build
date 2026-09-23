// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const { App, go, markRoute, local, device, archive, paintDevicePicker, listeners, deviceListeners } = vi.hoisted(() => ({
  App: { route: { name: "account", page: "settings" }, devices: [], viewDispose: null },
  go: vi.fn(), markRoute: vi.fn(), local: vi.fn(), device: vi.fn(), archive: vi.fn(), paintDevicePicker: vi.fn(), listeners: new Set(), deviceListeners: new Set(),
}));
vi.mock("../src/app.js", () => ({ App, go, markRoute }));
vi.mock("../src/devices.js", () => ({
  paintDevicePicker,
  readCachedDevices: async () => App.devices,
  onDevicesChanged: (listener) => {
    deviceListeners.add(listener);
    return () => deviceListeners.delete(listener);
  },
}));
vi.mock("../src/views/settings.js", () => ({ renderSettings: local }));
vi.mock("../src/views/deviceSettings.js", () => ({ renderDeviceSettings: device }));
vi.mock("../src/views/archive.js", () => ({ renderArchive: archive }));
vi.mock("../src/core/deviceContexts.js", () => ({ onDeviceStateChanged: (listener) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
} }));
import { isSettingsRoute, renderSettingsModal } from "../src/views/settingsModal.js";
import { rememberBridgeUpdateStatus, trackBridgeUpdateDevices } from "../src/core/bridgeUpdates.js";
import { wipeCache } from "../src/core/localCache.js";
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
// The modal hands its teardown BACK rather than claiming App.viewDispose: that
// slot belongs to the page underneath, which stays mounted while settings are
// open. So each case holds the handle the app holds.
let closeModal = null;
const openModal = (returnRoute) => {
  closeModal = renderSettingsModal(returnRoute);
};
beforeEach(async () => {
  vi.clearAllMocks();
  await wipeCache();
  deviceListeners.clear();
  document.body.innerHTML = '<div id="shell"><main id="root">Workspace content</main></div><div id="scrim"></div>';
  App.devices = [{ id: "a", name: "Laptop", status: "online" }, { id: "b", name: "Server", status: "offline" }];
  App.route = { name: "account", page: "settings" };
  local.mockImplementation(async ({ root }) => { root.innerHTML = '<button>Preference</button>'; });
  device.mockImplementation(async ({ root, deviceId }) => { root.textContent = deviceId; });
  archive.mockImplementation(({ root }) => { root.innerHTML = '<h1>Archive</h1>'; });
});
afterEach(() => { closeModal?.(); closeModal = null; App.viewDispose?.(); App.viewDispose = null; });
describe("settings modal", () => {
  it("marks the device with a release in the settings sidebar", async () => {
    trackBridgeUpdateDevices(App.devices);
    openModal();
    await rememberBridgeUpdateStatus("a", {
      running_version: "0.2.0", platform: "linux-x86_64", development_build: false,
      latest_release: { version: "0.3.0", tag: "bridge-v0.3.0", published_at: null },
      last_checked_at: "2026-09-23T12:00:00Z", state: "available", last_error: null,
      update_available: true, can_install: true,
    });
    await vi.waitFor(() => expect(document.querySelector('[data-settings-device="a"] .bridge-update-dot')).toBeTruthy());
    expect(document.querySelector('[data-settings-device="a"]').textContent).toContain("Update available");
    expect(document.querySelector('[data-settings-device="b"] .bridge-update-dot')).toBeNull();
  });
  it("lists local settings and devices over the existing surface, returning on close", async () => {
    const returnRoute = { name: "workspace", deviceId: "a", workspaceId: "w1" };
    openModal(returnRoute);
    await flush();
    expect(document.querySelector('[role="dialog"]').textContent).toContain("Local settings");
    expect(document.querySelectorAll('[data-settings-device]')).toHaveLength(2);
    expect(document.querySelector('#root').textContent).toBe("Workspace content");
    expect(document.querySelector('#shell').inert).toBe(true);
    document.querySelector('[data-settings-close]').click();
    expect(go).toHaveBeenCalledWith(returnRoute);
    closeModal(); closeModal = null;
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
    openModal();
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
  it("opens device deep links and lets nested sheets own Escape", async () => {
    App.route = { name: "device", id: "b" };
    openModal();
    await flush();
    expect(device.mock.calls[0][0].deviceId).toBe("b");
    document.querySelector('#scrim').classList.add("show");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(go).not.toHaveBeenCalled();
    document.querySelector('#scrim').classList.remove("show");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(go).toHaveBeenCalledWith({ name: "inbox" });
  });
  it("keeps keyboard focus in a nested sheet", () => {
    openModal();
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
  it("keeps archive inside the modal and disposes it when switching sections", async () => {
    const dispose = vi.fn();
    archive.mockImplementation(({ root, registerDispose }) => {
      root.innerHTML = '<h1>Archive</h1>';
      registerDispose(dispose);
    });
    App.route = { name: "account", page: "archive" };
    openModal();
    await flush();

    expect(document.querySelector('[role="dialog"] h1').textContent).toBe("Archive");
    expect(document.querySelector('.settings-archive').getAttribute('aria-current')).toBe('page');
    expect(archive).toHaveBeenCalledOnce();
    document.querySelector('[data-local]').click();
    expect(dispose).toHaveBeenCalledOnce();
    expect(markRoute).toHaveBeenLastCalledWith({ name: "account", page: "settings" });
  });

  it("keeps focus on Archive while marking it selected", async () => {
    openModal();
    await flush();
    const link = document.querySelector('.settings-archive');
    link.focus();
    link.click();
    await flush();

    expect(document.activeElement).toBe(document.querySelector('.settings-archive'));
    expect(document.activeElement.getAttribute('aria-current')).toBe('page');
  });

  it("returns from a direct archive route to the surface beneath the modal", async () => {
    const returnRoute = { name: "workspace", deviceId: "a", projectId: "p1", workspaceId: "w1" };
    App.route = { name: "account", page: "archive" };
    openModal(returnRoute);
    await flush();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(go).toHaveBeenCalledWith(returnRoute);
  });

  it("treats direct archive links as settings routes", () => {
    expect(isSettingsRoute({ name: "account", page: "archive" })).toBe(true);
    expect(isSettingsRoute({ name: "account", page: "devices" })).toBe(true);
    expect(isSettingsRoute({ name: "device", id: "a" })).toBe(true);
  });
});
