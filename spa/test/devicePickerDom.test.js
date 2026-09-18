// @vitest-environment jsdom
// The nav device picker: which machines the inbox shows, and the way to each
// machine's own settings page.
//
// It is a filter and nothing more. Picking a device narrows three lists on this
// browser; it does not move where creation goes, it opens no session and it
// leaves the reader exactly where they were standing.
import { beforeEach, describe, expect, it, vi } from "vitest";
const { App, go } = vi.hoisted(() => ({ App: {}, go: vi.fn() }));
vi.mock("../src/app.js", () => ({ App, go, DEVICE_FILTER_KEY: "build.deviceFilter" }));
vi.mock("../src/api.js", () => ({ fetchDevices: vi.fn() }));
vi.mock("../src/connection.js", () => ({
  syncDeviceRecoveryPresence: vi.fn(), deviceRecoverySnapshot: () => [], onDeviceRecoveryChanged: () => () => {},
  chooseCreationDevice: () => {},
  retireDevice: () => {},
  openDeviceSessions: vi.fn(),
  syncHome: vi.fn(),
  goOffline: vi.fn(),
  deviceWentAway: vi.fn(),
  connectDevice: vi.fn(async () => null),
  forgetHomeFollow: vi.fn(),
  forgetSecurityStops: vi.fn(),
  forgetRendezvousSockets: vi.fn(),
  securityStopText: () => "",
  openDeviceSettingsSession: vi.fn(),
}));
vi.mock("../src/core/inboxView.js", () => ({ inboxListRouteChanged: vi.fn(), mountInboxList: vi.fn(), setInboxView: vi.fn() }));
import { initDevicePicker, paintDevicePicker } from "../src/devices.js";
import { rememberDeviceFilter } from "../src/core/deviceFilter.js";
import { adoptBridgeSelection, adoptDeviceSession, contextFor, resetDeviceContexts, setContextOffline } from "../src/core/deviceContexts.js";

const choices = () => [...document.querySelectorAll(".device-picker-choice")];
const labelOf = (button) => button.querySelector("span").textContent;
const toggleLabel = () => document.querySelector(".device-picker-toggle > span").textContent;

beforeEach(() => {
  vi.clearAllMocks();
  go.mockReturnValue(true);
  document.body.className = "";
  localStorage.clear();
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });
  document.body.innerHTML = '<div id="devpick"></div>';
  Object.assign(App, { gated: false, deviceFilter: null, selectedDeviceId: "a", devices: [
    { id: "a", name: "Laptop", status: "online" }, { id: "b", name: "Desktop", status: "offline" },
  ] });
  resetDeviceContexts();
  initDevicePicker();
  paintDevicePicker();
});

/** A paired device this client has open. */
const deviceAnswering = (deviceId) =>
  adoptDeviceSession({ deviceId, call: async () => ({}), close: () => {}, peer: () => {}, onCarrier: () => {} });

describe("custom device picker", () => {
  it("reveals device settings by closing the rail on a phone", () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 390 });
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="a"]').click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  });
  it("keeps an online device's cog routing to settings", () => {
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="a"]').click();
    expect(go).toHaveBeenCalledWith({ name: "device", id: "a" });
    expect(App.deviceFilter).toBe(null);
    expect(document.querySelector(".device-picker-menu").hidden).toBe(true);
  });
  it('offers an "All devices" row, first and pressed while nothing is filtered', () => {
    document.querySelector(".device-picker-toggle").click();
    expect(choices().map(labelOf)).toEqual(["All devices", "Laptop", "Desktop"]);
    expect(choices()[0].dataset.filterDevice).toBe("");
    expect(choices().map((button) => button.getAttribute("aria-pressed"))).toEqual(["true", "false", "false"]);
    expect(toggleLabel()).toBe("All devices");
    // The account's own rows keep their way to that machine's settings; the
    // all-devices row is about no machine, so it has no cog.
    expect(choices()[0].closest(".device-picker-row").querySelector("[data-settings-device]")).toBeNull();
  });
  it("shows an offline icon instead of a settings control, while still allowing its filter", () => {
    document.querySelector(".device-picker-toggle").click();
    const row = choices()[2].closest(".device-picker-row");
    const offline = row.querySelector(".device-picker-offline");

    expect(row.querySelector('[data-settings-device="b"]')).toBeNull();
    expect(offline.getAttribute("aria-label")).toBe("Device offline");
    expect(offline.getAttribute("title")).toBe("Device offline");
    expect(offline.tabIndex).toBe(-1);

    choices()[2].click();
    expect(App.deviceFilter).toBe("b");
    expect(toggleLabel()).toBe("Desktop");
  });
  // The rows in the rail say why a machine cannot be asked anything; the picker
  // is the same account list and says the same word, so a bridge answering in a
  // shape this app cannot read does not read as plainly online here.
  it("says why a machine cannot be asked, in the word its rows wear", () => {
    deviceAnswering("a");
    adoptBridgeSelection(contextFor("a"), { version: "0.9.0", unsupported: "bridge" }, null);
    paintDevicePicker();
    document.querySelector(".device-picker-toggle").click();
    expect(choices().map(labelOf)).toEqual(["All devices", "Laptop (update)", "Desktop"]);
    expect(document.querySelector('[data-settings-device="a"]')).not.toBeNull();
  });

  // The account list is one read behind the session: a machine whose session
  // has dropped is away on the rail the moment it goes, so it is away here too.
  it("calls a machine away once its own session has gone, whatever the list says", () => {
    deviceAnswering("a");
    setContextOffline("a", { offline: true });
    expect(labelOf(choices()[1])).toBe("Laptop");
    expect(choices()[1].closest(".device-picker-row").querySelector('[data-settings-device="a"]')).toBeNull();
    expect(choices()[1].closest(".device-picker-row").querySelector(".device-picker-offline")).not.toBeNull();
  });

  it("restores the cog as a lost connection reconnects", () => {
    deviceAnswering("a");
    setContextOffline("a", { offline: true });
    expect(document.querySelector('[data-settings-device="a"]')).toBeNull();

    setContextOffline("a", { offline: false });
    expect(document.querySelector('[data-settings-device="a"]')).not.toBeNull();
  });

  it("keeps an open menu and its row focused when that device goes offline", () => {
    deviceAnswering("a");
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="a"]').focus();

    setContextOffline("a", { offline: true });

    expect(document.querySelector(".device-picker-menu").hidden).toBe(false);
    expect(document.querySelector('[data-settings-device="a"]')).toBeNull();
    expect(document.activeElement.dataset.filterDevice).toBe("a");
  });

  it("does not route a stale settings cog after the device becomes offline", () => {
    document.querySelector(".device-picker-toggle").click();
    App.devices[0].status = "offline";
    document.querySelector('[data-settings-device="a"]').click();

    expect(go).not.toHaveBeenCalled();
    expect(document.querySelector('[data-settings-device="a"]')).toBeNull();
    expect(document.querySelector(".device-picker-offline")).not.toBeNull();
  });

  it("uses the same offline icon for a blocked connection", () => {
    deviceAnswering("a");
    setContextOffline("a", { offline: true, blocked: "timeout" });

    expect(labelOf(choices()[1])).toBe("Laptop");
    expect(choices()[1].closest(".device-picker-row").querySelector(".device-picker-offline")).not.toBeNull();
  });

  // A machine the account calls online that this client has not opened yet is
  // not away: the picker says what the account list says until a context of its
  // own says otherwise.
  it("says nothing about a machine it has not opened yet", () => {
    expect(labelOf(choices()[1])).toBe("Laptop");
  });

  it("picking a device sets the filter and does not touch selectedDeviceId", () => {
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-filter-device="b"]').click();

    expect(App.deviceFilter).toBe("b");
    expect(localStorage.getItem("build.deviceFilter")).toBe("b");
    expect(App.selectedDeviceId).toBe("a"); // where creation goes is said elsewhere
    expect(toggleLabel()).toBe("Desktop");
    expect(document.querySelector(".device-picker-menu").hidden).toBe(true);

    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-filter-device=""]').click();

    expect(App.deviceFilter).toBe(null);
    expect(localStorage.getItem("build.deviceFilter")).toBe(null);
    expect(toggleLabel()).toBe("All devices");
  });
  it("names the device the rail is filtered to, until that device leaves the account", async () => {
    rememberDeviceFilter("b");
    paintDevicePicker();
    expect(toggleLabel()).toBe("Desktop");

    const { fetchDevices } = await import("../src/api.js");
    fetchDevices.mockResolvedValue([{ id: "a", name: "Laptop", status: "online" }]);
    const { refreshDevices } = await import("../src/devices.js");
    await refreshDevices();

    // Filtering to a device the account no longer lists would empty the rail
    // with nothing on screen to say why.
    expect(App.deviceFilter).toBe(null);
    expect(toggleLabel()).toBe("All devices");
  });
  it("supports arrow navigation and Escape returns focus to the trigger", () => {
    const toggle = document.querySelector(".device-picker-toggle");
    toggle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.filterDevice).toBe(""); // All devices, first
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.filterDevice).toBe("a");
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.settingsDevice).toBe("a");
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.activeElement).toBe(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");

    toggle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.filterDevice).toBe("b");
    // Native button activation is what Enter/Space does in the browser. The
    // static offline icon is absent from this tab sequence.
    document.activeElement.click();
    expect(App.deviceFilter).toBe("b");
  });
});
