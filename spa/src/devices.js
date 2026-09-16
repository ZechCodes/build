// The device store, the presence poll, and the nav device picker.
//
// Presence is the api's (spec rule 6): `GET /api/devices` is the only place a
// machine's status comes from — the relay says nothing about which machines are
// up — so this module re-reads it on a cadence while the app is open, and at
// once when the tab comes back to the front. Two things follow every read: a
// machine that is online with no live session is opened (a late device joining,
// or one that came back), and a machine this client holds that the account no
// longer lists online is marked away. A live connection failing is the other,
// faster signal, and it is the connection layer's (connection.js).

import { $ } from "./dom.js";
import { esc } from "./core/text.js";
import { canAnswer, contextFor, knownContexts, onDeviceStateChanged } from "./core/deviceContexts.js";
import { deviceAwayWord } from "./core/deviceAway.js";
import { ICON_CHEVRON_DOWN, ICON_SETTINGS, ICON_WIFI_OFF } from "./core/icons.js";
import { App } from "./app.js";
import { goFromInbox } from "./core/inboxShell.js";
import { fetchDevices } from "./api.js";
import { deviceNameOf } from "./core/devicePolicy.js";
import { rememberDeviceFilter } from "./core/deviceFilter.js";
import { deviceWentAway, openDeviceSessions, syncHome } from "./connection.js";

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

/** How often the account's list is re-read while the app is open. The gate has
 *  a quicker one of its own (3 s) for the screen that is waiting on a machine;
 *  this is the cadence for an app that is already standing on one. */
const PRESENCE_INTERVAL_MS = 15000;

let presenceTimer = null;
let onVisibilityChange = null;

/**
 * Follow the account's presence while the app is open.
 *
 * Re-entrant: asking again re-arms the one poll rather than starting a second.
 * The gate stops it whenever it takes the app back — a poll left running would
 * open sessions behind a screen that is already asking for them.
 */
export function watchPresence({ intervalMs = PRESENCE_INTERVAL_MS } = {}) {
  stopWatchingPresence();
  presenceTimer = setInterval(() => readPresence(), intervalMs);
  // A tab that was in the background missed every tick: what it shows is as old
  // as the last one, so the first thing it does on the way back is read.
  onVisibilityChange = () => {
    if (document.visibilityState !== "hidden") readPresence();
  };
  document.addEventListener("visibilitychange", onVisibilityChange);
}

/** Stop following it (the gate took the app back, the account signed out). */
export function stopWatchingPresence() {
  clearInterval(presenceTimer);
  presenceTimer = null;
  if (onVisibilityChange) document.removeEventListener("visibilitychange", onVisibilityChange);
  onVisibilityChange = null;
}

/**
 * One read of the account's list, and everything that follows from it.
 *
 * A read that fails is news about nothing: the api is unreachable, which says
 * nothing about any machine, and marking them all away over a flaky network
 * would empty the app. The next tick reads again.
 */
export async function readPresence() {
  try {
    await refreshDevices();
  } catch {
    return null;
  }
  markWhatTheAccountNoLongerLists();
  openDeviceSessions(); // a late device, a machine that came back, one paired elsewhere
  // Which device is home is what these statuses say: the picked device dropping
  // hands home to the first that is still online, and its coming back takes it
  // straight back.
  syncHome();
  return App.devices;
}

/** Every machine this client is holding that the account no longer calls
 *  online: its bridge stopped saying it was there. */
function markWhatTheAccountNoLongerLists() {
  for (const context of knownContexts()) {
    if (deviceFor(context.deviceId)?.status !== "online") deviceWentAway(context.deviceId);
  }
}

const ALL_DEVICES = "All devices";

export function paintDevicePicker() {
  const picker = $("#devpick");
  if (!picker) return;
  const menu = picker.querySelector(".device-picker-menu");
  const wasOpen = Boolean(menu && !menu.hidden);
  const focused = focusedPickerControl(picker);
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
  setPickerOpen(picker, wasOpen);
  restorePickerFocus(picker, focused);
}

/** Which picker control had focus before a repaint. Keeping this as data rather
 * than an element lets a state change replace a stale settings cog with that
 * row's still-live filter control. */
function focusedPickerControl(picker) {
  const button = document.activeElement?.closest?.("button");
  if (!button || !picker.contains(button)) return null;
  if (button.classList.contains("device-picker-toggle")) return { kind: "toggle" };
  if (button.dataset.settingsDevice) return { kind: "settings", deviceId: button.dataset.settingsDevice };
  if (button.hasAttribute("data-filter-device")) return { kind: "filter", deviceId: button.dataset.filterDevice };
  return null;
}

function restorePickerFocus(picker, focused) {
  if (!focused) return;
  const buttons = [...picker.querySelectorAll("button")];
  const matching = (name, deviceId) => buttons.find((button) => button.dataset[name] === deviceId);
  const target = focused.kind === "toggle"
    ? picker.querySelector(".device-picker-toggle")
    : focused.kind === "settings"
      ? matching("settingsDevice", focused.deviceId) || matching("filterDevice", focused.deviceId)
      : matching("filterDevice", focused.deviceId);
  target?.focus();
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
  const offline = deviceIsOffline(device);
  return `<div class="device-picker-row">
    ${choiceHtml(device.id, deviceLabel(device), filter === device.id)}
    ${offline
      ? `<span class="device-picker-offline" role="img" aria-label="Device offline" title="Device offline">${ICON_WIFI_OFF}</span>`
      : `<button type="button" class="device-picker-settings" data-settings-device="${esc(device.id)}" aria-label="Settings for ${esc(device.name)}" title="Settings for ${esc(device.name)}"><span aria-hidden="true">${ICON_SETTINGS}</span></button>`}
  </div>`;
}

/** What every row is picked by: the device the rail is to show, with the
 *  account's own row naming no device at all. */
function choiceHtml(deviceId, label, pressed) {
  return `<button type="button" class="device-picker-choice" data-filter-device="${esc(deviceId)}" aria-pressed="${pressed}">
      <span>${esc(label)}</span><span aria-hidden="true">${pressed ? "✓" : ""}</span>
    </button>`;
}

/** How a machine reads in the picker: the account's name for it, and — when it
 *  cannot be asked anything — the same one word its rows in the rail wear, which
 *  says WHY it cannot (core/deviceNotice.js). A machine this client has not
 *  opened yet has nothing of its own to say, so the account list speaks for it. */
function deviceLabel(device) {
  const context = contextFor(device.id);
  if (deviceIsOffline(device)) return device.name;
  return context && !canAnswer(context) ? `${device.name} (${deviceAwayWord(context)})` : device.name;
}

/** A connection can go away before the next account-list read arrives. Keep
 * the picker from offering settings against either signal of an unavailable
 * machine. Update-only contexts remain available and retain their settings. */
function deviceIsOffline(device) {
  return device.status !== "online" || Boolean(contextFor(device.id)?.offline);
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
    const device = deviceFor(button.dataset.settingsDevice);
    if (!device || deviceIsOffline(device)) {
      paintDevicePicker();
      return;
    }
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
  const stopWatchingDeviceState = onDeviceStateChanged(paintDevicePicker);
  removePickerListeners = () => {
    document.removeEventListener("click", dismiss);
    document.removeEventListener("focusin", dismiss);
    stopWatchingDeviceState();
  };
}
