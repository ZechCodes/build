// The device store + the nav device picker. Statuses come from GET /api/devices
// and are patched live by the relay's device_key / device_offline pushes.

import { $ } from "./dom.js";
import { esc } from "./core/text.js";
import { ICON_CHEVRON_DOWN, ICON_SETTINGS } from "./core/icons.js";
import { App } from "./app.js";
import { goFromInbox } from "./core/inboxShell.js";
import { fetchDevices } from "./api.js";
import { deviceNameOf } from "./core/devicePolicy.js";
import { openDeviceSessions, setHomeDevice, syncHome } from "./connection.js";

export async function refreshDevices() {
  App.devices = await fetchDevices();
  paintDevicePicker();
  return App.devices;
}

/** The account's entry for one device, as the list last saw it. */
const deviceFor = (deviceId) => App.devices.find((device) => device.id === deviceId) || null;

export function deviceName(deviceId) {
  return deviceNameOf(App.devices, deviceId);
}

/**
 * The api-pinned transport key for a device — the E2EE trust anchor sessions
 * seal to (never the relay-pushed key). Unknown devices trigger one refresh
 * (e.g. approved in another tab); still-unknown devices return null and the
 * session layer refuses to connect to them.
 */
export async function pinnedDeviceTransportKey(deviceId) {
  const pinnedKey = () => deviceFor(deviceId)?.transport_public_key_b64 || null;
  const known = pinnedKey();
  if (known) return known;
  await refreshDevices();
  return pinnedKey();
}

/** Patch one device's status and repaint the picker that reads it. Says whether
 *  this was news; a device the list has never heard of is nobody to patch. */
function markDevice(deviceId, status) {
  const device = deviceFor(deviceId);
  if (!device || device.status === status) return false;
  device.status = status;
  paintDevicePicker();
  // Which device is home is what these statuses say: the picked device dropping
  // hands home to the first that is still online, and its coming back takes it
  // straight back.
  syncHome();
  return true;
}

export function markDeviceOnline(deviceId) {
  if (!deviceFor(deviceId)) {
    // A device we have not seen yet (approved elsewhere) — read the list, then
    // join it like any other. Reading alone would leave it online in the picker
    // and contributing no rows until it next reconnected.
    refreshDevices()
      .then(() => openDeviceSessions())
      .catch(() => {
        /* the account list is unreachable; the next push tries again */
      });
    return;
  }
  // A device that came up after boot joins the account's inbox here, without a
  // reload: every online device with no live session is opened.
  if (markDevice(deviceId, "online")) openDeviceSessions();
}

export function markDeviceOffline(deviceId) {
  markDevice(deviceId, "offline");
}

export function paintDevicePicker() {
  const picker = $("#devpick");
  if (!picker) return;
  picker.hidden = App.gated || App.devices.length === 0;
  if (picker.hidden) return;
  const current = App.session?.deviceId || App.selectedDeviceId || "";
  const selected = App.devices.find((device) => device.id === current);
  const label = selected ? deviceLabel(selected) : "Choose a device";
  picker.innerHTML = `
    <button class="device-picker-toggle" type="button" aria-expanded="false" aria-controls="device-picker-menu" title="Which device runs your tasks">
      <span>${esc(label)}</span><span aria-hidden="true">${ICON_CHEVRON_DOWN}</span>
    </button>
    <div class="device-picker-menu" id="device-picker-menu" aria-label="Devices" hidden>
      ${App.devices.map((device) => deviceRow(device, current)).join("")}
    </div>
    <div class="device-picker-error" role="status"></div>`;
}

function deviceLabel(device) {
  return `${device.name}${device.status === "online" ? "" : " (offline)"}`;
}

function deviceRow(device, current) {
  const label = deviceLabel(device);
  return `<div class="device-picker-row">
    <button type="button" class="device-picker-choice" data-select-device="${esc(device.id)}" aria-pressed="${device.id === current}">
      <span>${esc(label)}</span><span aria-hidden="true">${device.id === current ? "✓" : ""}</span>
    </button>
    <button type="button" class="device-picker-settings" data-settings-device="${esc(device.id)}" aria-label="Settings for ${esc(device.name)}" title="Settings for ${esc(device.name)}"><span aria-hidden="true">${ICON_SETTINGS}</span></button>
  </div>`;
}

function setPickerOpen(picker, open) {
  const menu = picker.querySelector(".device-picker-menu");
  if (!menu) return;
  menu.hidden = !open;
  picker.querySelector(".device-picker-toggle").setAttribute("aria-expanded", String(open));
}

let movingHome = false;
async function selectDevice(picker, deviceId) {
  if (movingHome) return;
  movingHome = true;
  setPickerOpen(picker, false);
  try {
    await setHomeDevice(deviceId);
    paintDevicePicker();
  } catch {
    paintDevicePicker();
    picker.querySelector(".device-picker-error").textContent = "Device unreachable. Try again when it is online.";
  } finally {
    movingHome = false;
  }
}

function pickerClick(event, picker) {
  const button = event.target.closest("button");
  if (!button) return;
  if (button.classList.contains("device-picker-toggle")) {
    setPickerOpen(picker, button.getAttribute("aria-expanded") !== "true");
  } else if (button.dataset.settingsDevice) {
    setPickerOpen(picker, false);
    goFromInbox({ name: "device", id: button.dataset.settingsDevice });
  } else if (button.dataset.selectDevice) {
    void selectDevice(picker, button.dataset.selectDevice);
  }
}

function pickerKeydown(event, picker) {
  if (event.key === "Escape") {
    setPickerOpen(picker, false);
    picker.querySelector(".device-picker-toggle").focus();
    event.preventDefault();
    return;
  }
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  setPickerOpen(picker, true);
  const buttons = [...picker.querySelectorAll(".device-picker-menu button")];
  const index = buttons.indexOf(document.activeElement);
  const next = pickerFocusIndex(event.key, index, buttons.length);
  buttons[next]?.focus();
}

function pickerFocusIndex(key, index, length) {
  if (key === "Home") return 0;
  if (key === "End" || index < 0 && key === "ArrowUp") return length - 1;
  return (index + (key === "ArrowUp" ? -1 : 1) + length) % length;
}

let removePickerListeners = () => {};
export function initDevicePicker() {
  removePickerListeners();
  const picker = $("#devpick");
  picker.onclick = (event) => pickerClick(event, picker);
  picker.onkeydown = (event) => pickerKeydown(event, picker);
  const dismiss = (event) => {
    if (!picker.contains(event.target)) setPickerOpen(picker, false);
  };
  document.addEventListener("click", dismiss);
  document.addEventListener("focusin", dismiss);
  removePickerListeners = () => {
    document.removeEventListener("click", dismiss);
    document.removeEventListener("focusin", dismiss);
  };
}
