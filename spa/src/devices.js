// The device store + the nav device picker. Statuses come from GET /api/devices
// and are patched live by the relay's device_key / device_offline pushes.

import { $ } from "./dom.js";
import { esc } from "./core/text.js";
import { App } from "./app.js";
import { fetchDevices } from "./api.js";
import { switchDevice, setConn } from "./connection.js";

export async function refreshDevices() {
  App.devices = await fetchDevices();
  paintDevicePicker();
  return App.devices;
}

export function deviceName(deviceId) {
  return App.devices.find((d) => d.id === deviceId)?.name || null;
}

/**
 * The api-pinned transport key for a device — the E2EE trust anchor sessions
 * seal to (never the relay-pushed key). Unknown devices trigger one refresh
 * (e.g. approved in another tab); still-unknown devices return null and the
 * session layer refuses to connect to them.
 */
export async function pinnedDeviceTransportKey(deviceId) {
  const pinnedKey = () => App.devices.find((d) => d.id === deviceId)?.transport_public_key_b64 || null;
  const known = pinnedKey();
  if (known) return known;
  await refreshDevices();
  return pinnedKey();
}

export function markDeviceOnline(deviceId) {
  const device = App.devices.find((d) => d.id === deviceId);
  if (!device) {
    // A device we have not seen yet (approved elsewhere) — refresh the list.
    refreshDevices();
    return;
  }
  if (device.status !== "online") {
    device.status = "online";
    paintDevicePicker();
  }
}

export function markDeviceOffline(deviceId) {
  const device = App.devices.find((d) => d.id === deviceId);
  if (device && device.status !== "offline") {
    device.status = "offline";
    paintDevicePicker();
  }
}

export function paintDevicePicker() {
  const picker = $("#devpick");
  if (App.gated || App.devices.length === 0) {
    picker.hidden = true;
    return;
  }
  picker.hidden = false;
  const current = App.session?.deviceId || App.selectedDeviceId || "";
  picker.innerHTML = App.devices
    .map(
      (d) => `<option value="${esc(d.id)}" ${d.id === current ? "selected" : ""}>` +
        `${esc(d.name)}${d.status === "online" ? "" : " (offline)"}</option>`,
    )
    .join("");
}

export function initDevicePicker() {
  $("#devpick").onchange = async (event) => {
    const deviceId = event.target.value;
    try {
      await switchDevice(deviceId);
    } catch {
      paintDevicePicker(); // revert the selection
      setConn('<span class="dot" style="background:#d29922"></span>device unreachable');
      setTimeout(() => {
        if (!App.offline) setConn('<span class="dot"></span>connected');
      }, 2500);
    }
  };
}
