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
  chooseCreationDevice: () => {},
  retireDevice: () => {},
  openDeviceSessions: vi.fn(),
  syncHome: vi.fn(),
  goOffline: vi.fn(),
  forgetHomeFollow: vi.fn(),
  forgetSecurityStops: vi.fn(),
  securityStopText: () => "",
  openDeviceSettingsSession: vi.fn(),
}));
vi.mock("../src/core/inboxView.js", () => ({ inboxListRouteChanged: vi.fn(), mountInboxList: vi.fn(), setInboxView: vi.fn() }));
import { initDevicePicker, paintDevicePicker } from "../src/devices.js";
import { rememberDeviceFilter } from "../src/core/deviceFilter.js";

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
  initDevicePicker();
  paintDevicePicker();
});
describe("custom device picker", () => {
  it("reveals device settings by closing the rail on a phone", () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 390 });
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="b"]').click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  });
  it("the cog still routes to settings", () => {
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="b"]').click();
    expect(go).toHaveBeenCalledWith({ name: "device", id: "b" });
    expect(App.deviceFilter).toBe(null);
    expect(document.querySelector(".device-picker-menu").hidden).toBe(true);
  });
  it('offers an "All devices" row, first and pressed while nothing is filtered', () => {
    document.querySelector(".device-picker-toggle").click();
    expect(choices().map(labelOf)).toEqual(["All devices", "Laptop", "Desktop (offline)"]);
    expect(choices()[0].dataset.filterDevice).toBe("");
    expect(choices().map((button) => button.getAttribute("aria-pressed"))).toEqual(["true", "false", "false"]);
    expect(toggleLabel()).toBe("All devices");
    // The account's own rows keep their way to that machine's settings; the
    // all-devices row is about no machine, so it has no cog.
    expect(choices()[0].closest(".device-picker-row").querySelector("[data-settings-device]")).toBeNull();
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
  });
});
