// The device store + the nav device picker. Statuses come from GET /api/devices
// and are patched live by the relay's device_key / device_offline pushes.

import { $ } from "./dom.js";
import { esc } from "./core/text.js";
import { ICON_CHEVRON_DOWN, ICON_SETTINGS } from "./core/icons.js";
import { App } from "./app.js";
import { goFromInbox } from "./core/inboxShell.js";
import { fetchDevices } from "./api.js";
import { deviceNameOf } from "./core/devicePolicy.js";
import { rememberDeviceFilter } from "./core/deviceFilter.js";
import { openDeviceSessions, syncHome } from "./connection.js";

export async function refreshDevices() {
  App.devices = await fetchDevices();
  forgetFilterOnMissingDevice();
  paintDevicePicker();
  return App.devices;
}

/** A rail filtered to a device the account no longer lists would show nothing
 *  at all, with nothing on screen to say why. The account is what the picker is
 *  a filter over, so a device leaving it takes the filter with it. */
function forgetFilterOnMissingDevice() {
  if (App.deviceFilter && !deviceFor(App.deviceFilter)) rememberDeviceFilter(null);
}

/** The account's entry for one device, as the list last saw it. */
const deviceFor = (deviceId) => App.devices.find((device) => device.id === deviceId) || null;

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

const ALL_DEVICES = "All devices";

export function paintDevicePicker() {
  const picker = $("#devpick");
  if (!picker) return;
  picker.hidden = App.gated || App.devices.length === 0;
  if (picker.hidden) return;
  // The filter is over the account's own list, so the toggle says what the rail
  // is showing: one machine by name, or all of them.
  const label = deviceNameOf(App.devices, App.deviceFilter) || ALL_DEVICES;
  picker.innerHTML = `
    <button class="device-picker-toggle" type="button" aria-expanded="false" aria-controls="device-picker-menu" title="Which devices the inbox shows">
      <span>${esc(label)}</span><span aria-hidden="true">${ICON_CHEVRON_DOWN}</span>
    </button>
    <div class="device-picker-menu" id="device-picker-menu" aria-label="Devices" hidden>
      ${pickerRowsHtml()}
    </div>`;
}

/** The rows the menu offers: the all-devices choice, then one row per machine.
 *  Each kind paints itself, so the cog a machine's row carries is not a
 *  condition inside one renderer. */
function pickerRowsHtml() {
  const filter = App.deviceFilter || null;
  return [allDevicesRowHtml(filter), ...App.devices.map((device) => deviceRowHtml(device, filter))].join("");
}

/** The account as a whole, which is what the rail shows until a machine is
 *  picked. It is about no machine, so it has no settings cog. */
function allDevicesRowHtml(filter) {
  return `<div class="device-picker-row">
    ${choiceHtml("", ALL_DEVICES, filter === null)}
  </div>`;
}

function deviceRowHtml(device, filter) {
  return `<div class="device-picker-row">
    ${choiceHtml(device.id, deviceLabel(device), filter === device.id)}
    <button type="button" class="device-picker-settings" data-settings-device="${esc(device.id)}" aria-label="Settings for ${esc(device.name)}" title="Settings for ${esc(device.name)}"><span aria-hidden="true">${ICON_SETTINGS}</span></button>
  </div>`;
}

/** What every row is picked by: the device the rail is to show, with the
 *  account's own row naming no device at all. */
function choiceHtml(deviceId, label, pressed) {
  return `<button type="button" class="device-picker-choice" data-filter-device="${esc(deviceId)}" aria-pressed="${pressed}">
      <span>${esc(label)}</span><span aria-hidden="true">${pressed ? "✓" : ""}</span>
    </button>`;
}

function deviceLabel(device) {
  return `${device.name}${device.status === "online" ? "" : " (offline)"}`;
}

function setPickerOpen(picker, open) {
  const menu = picker.querySelector(".device-picker-menu");
  if (!menu) return;
  menu.hidden = !open;
  picker.querySelector(".device-picker-toggle").setAttribute("aria-expanded", String(open));
}

/** Show one machine's work, or every machine's. Nothing is connected and
 *  nothing is moved: the three lists are repainted from what the devices have
 *  already said (core/deviceFilter.js), and the reader stays where they are. */
function chooseDeviceFilter(picker, deviceId) {
  setPickerOpen(picker, false);
  rememberDeviceFilter(deviceId);
  paintDevicePicker();
}

function pickerClick(event, picker) {
  const button = event.target.closest("button");
  if (!button) return;
  if (button.classList.contains("device-picker-toggle")) {
    setPickerOpen(picker, button.getAttribute("aria-expanded") !== "true");
  } else if (button.dataset.settingsDevice) {
    setPickerOpen(picker, false);
    goFromInbox({ name: "device", id: button.dataset.settingsDevice });
  } else if (button.hasAttribute("data-filter-device")) {
    // The all-devices row names no device, which is what "no filter" is.
    chooseDeviceFilter(picker, button.dataset.filterDevice || null);
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
