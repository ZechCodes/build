// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
const { App, go, switchDevice } = vi.hoisted(() => ({ App: {}, go: vi.fn(), switchDevice: vi.fn() }));
vi.mock("../src/app.js", () => ({ App, go }));
vi.mock("../src/api.js", () => ({ fetchDevices: vi.fn() }));
vi.mock("../src/connection.js", () => ({ switchDevice, setConn: vi.fn() }));
vi.mock("../src/core/inboxView.js", () => ({ inboxListRouteChanged: vi.fn(), mountInboxList: vi.fn(), setInboxView: vi.fn() }));
import { initDevicePicker, paintDevicePicker } from "../src/devices.js";
beforeEach(() => {
  vi.clearAllMocks();
  go.mockReturnValue(true);
  document.body.className = "";
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });
  document.body.innerHTML = '<div id="devpick"></div>';
  Object.assign(App, { gated: false, session: { deviceId: "a" }, devices: [
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
  it("offers a separate settings action for every device without switching", () => {
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-settings-device="b"]').click();
    expect(go).toHaveBeenCalledWith({ name: "device", id: "b" });
    expect(switchDevice).not.toHaveBeenCalled();
    expect(document.querySelector(".device-picker-menu").hidden).toBe(true);
  });
  it("switches devices from the device action", async () => {
    document.querySelector(".device-picker-toggle").click();
    document.querySelector('[data-select-device="b"]').click();
    expect(switchDevice).toHaveBeenCalledWith("b");
  });
  it("supports arrow navigation and Escape returns focus to the trigger", () => {
    const toggle = document.querySelector(".device-picker-toggle");
    toggle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.selectDevice).toBe("a");
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement.dataset.settingsDevice).toBe("a");
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.activeElement).toBe(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
  });
});
