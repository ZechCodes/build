// Session lifecycle: open, adopt, degrade offline, resume, switch devices.
//
// The relay pushes device_offline the instant our bridge drops; the socket's
// own close event covers relay/network loss. Degrade quietly: banner + amber
// dot, freeze the current view (the polls fail fast and skip re-rendering, so
// in-progress typing/reading is untouched), pause bridge-backed actions — then
// resume silently the moment the device is back.

import * as transport from "@build/secure-transport";
import { $ } from "./dom.js";
import { RELAY_URL } from "./config.js";
import { openRelaySession } from "./core/session.js";
import { fetchGatewayToken } from "./api.js";
import { App, render, rememberSelectedDevice } from "./app.js";
import { deviceName, markDeviceOnline, markDeviceOffline, paintDevicePicker } from "./devices.js";

export function setConn(html) {
  $("#conn").innerHTML = html;
}

export function openAppSession({ preferDeviceId = null, waitForDevice = false } = {}) {
  return openRelaySession({
    relayUrl: RELAY_URL,
    transport,
    WebSocketImpl: WebSocket,
    fetchToken: fetchGatewayToken,
    preferDeviceId,
    waitForDevice,
    isPaused: () => App.offline,
    onDeviceKey: markDeviceOnline,
    onDeviceOffline: markDeviceOffline,
    onLost: goOffline,
  });
}

export function adoptSession(session) {
  App.session = session;
  App.call = session.call;
  paintDevicePicker();
}

function restoreOnline() {
  App.offline = false;
  document.body.classList.remove("offline");
  $("#offbar").hidden = true;
  setConn('<span class="dot"></span>connected');
}

let reconnectTimer = null;
let reconnectDelay = 0;

export function goOffline() {
  if (App.offline) return;
  App.offline = true;
  try {
    App.session?.close();
  } catch {
    /* already gone */
  }
  document.body.classList.add("offline");
  const name = deviceName(App.session?.deviceId) || "Your device";
  $("#offbar-text").textContent = `${name} went offline — reconnecting automatically.`;
  $("#offbar").hidden = false;
  setConn('<span class="dot" style="background:#d29922"></span>reconnecting…');
  resume();
}

export async function resume() {
  if (!App.offline || App._resuming) return;
  App._resuming = true;
  try {
    // Blocks until the device is online: the fresh authenticated socket receives
    // the relay's device_key push the moment a bridge returns. An explicit device
    // choice is honored; otherwise any of the user's devices brings us back.
    const session = await openAppSession({
      preferDeviceId: App.selectedDeviceId,
      waitForDevice: true,
    });
    if (!App.offline) {
      // Someone else (a device switch) already restored us while we waited.
      session.close();
      return;
    }
    adoptSession(session);
    restoreOnline();
    reconnectDelay = 0;
    // The board holds no user input, so refreshing it is safe; the task view's
    // own poll resumes and its key-diffing preserves in-progress work.
    if (App.route.name === "board") render();
  } catch {
    // Relay unreachable — retry with backoff until it's back.
    reconnectDelay = Math.min((reconnectDelay || 1000) * 2, 15000);
    clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(resume, reconnectDelay);
  } finally {
    App._resuming = false;
  }
}

/** Re-target the app at another device: open the new session first, then swap. */
export async function switchDevice(deviceId) {
  rememberSelectedDevice(deviceId);
  if (App.session?.deviceId === deviceId && !App.offline) return;
  const previous = App.session;
  const wasOffline = App.offline;
  App.offline = false; // let the fresh session's calls through
  let session;
  try {
    session = await openAppSession({ preferDeviceId: deviceId, waitForDevice: false });
  } catch (error) {
    App.offline = wasOffline;
    throw error;
  }
  try {
    previous?.close();
  } catch {
    /* already gone */
  }
  adoptSession(session);
  restoreOnline();
  render();
}
