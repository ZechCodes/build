// One machine's settings: everything the bridge on it owns — its projects,
// where they are kept, and how agents run there.
//
// The page keeps its own transport so browsing another machine cannot redirect
// workspace requests, or a late folder selection, to a different host. That is
// also why every panel here is handed this page's caller: the account page holds
// what is the account's, and every answer on this one belongs to this machine.
import { renameDevice, revokeDevice } from "../api.js";
import { confirmAction } from "../core/confirm.js";
import { $ } from "../dom.js";
import { deviceOfflineText, esc } from "../core/text.js";
import { deviceOfflineNotice } from "../core/deviceNotice.js";
import { contextFor } from "../core/deviceContexts.js";
import { App } from "../app.js";
import { cacheRenamedDevice, cacheRevokedDevice, onDevicesChanged } from "../devices.js";
import { openDeviceSettingsSession, retireDevice } from "../connection.js";
import { openBrowser } from "../sheets/browser.js";
import { standUpDevicePanels } from "./devicePanels.js";
import { deviceSettingsAddress, watchSettingsRecord } from "../core/settingsRecords.js";
import { mountBridgeUpdatePanel } from "../core/bridgeUpdatePanel.js";
import { fieldTraits } from "../core/fieldTraits.js";

export async function renderDeviceSettings({ root = $("#root"), deviceId = App.route.id, embedded = false, registerDispose = (dispose) => { App.viewDispose = dispose; }, onDeviceDeactivated } = {}) {
  let device = App.devices.find((item) => item.id === deviceId);
  root.classList.add("device-settings");
  if (!device) {
    root.innerHTML = '<div class="board-head"><h1>Device not found</h1></div><p>This device is no longer paired with your account.</p><a class="btn" href="#/account/settings">Account settings</a>';
    return;
  }
  root.innerHTML = `
    ${embedded ? "" : '<a class="btn mini" href="#/account/settings">Local settings</a>'}
    <div class="board-head"><div><h1 id="device-settings-title">${esc(device.name)} settings</h1><p>The projects this machine holds, and how agents run on it.</p></div></div>
    <div class="panel">
      <h3>Device name</h3>
      <form id="device-label-form">
        <div class="field"><label for="device-label">Name</label>
        <div class="projfolder"><input id="device-label" ${fieldTraits("line")} value="${esc(device.name)}" maxlength="255" required>
          <button class="btn" type="submit">Save</button></div>
        </div>
        <p id="device-label-status" role="status" aria-live="polite"></p>
      </form>
    </div>
    <div id="device-projects-panel"></div>
    <div class="panel">
      <h3>Projects folder</h3>
      <p class="dim">New projects and cloned repositories will be kept in this folder on <span id="device-projects-device-name">${esc(device.name)}</span>. Existing projects stay where they are.</p>
      <div class="projfolder"><code id="device-projects-path">Loading…</code>
        <button class="btn" id="device-projects-change" disabled>Choose folder…</button></div>
      <p id="device-settings-status" role="status" aria-live="polite"></p>
      <button class="btn mini" id="device-settings-retry" hidden>Retry</button>
    </div>
    <div id="device-updates-panel"></div>
    <div id="device-bridge-panels"></div>
    <div class="panel danger-zone">
      <h3>Deactivate device</h3>
      <p class="dim">Remove this device's access to your account. It will need to be paired again before it can reconnect.</p>
      <button class="btn danger" id="device-deactivate" type="button">Deactivate device…</button>
      <p id="device-deactivate-status" role="status" aria-live="polite"></p>
    </div>`;
  const pathLabel = root.querySelector("#device-projects-path");
  const change = root.querySelector("#device-projects-change");
  const status = root.querySelector("#device-settings-status");
  const retry = root.querySelector("#device-settings-retry");
  const nameForm = root.querySelector("#device-label-form");
  const nameInput = root.querySelector("#device-label");
  const nameStatus = root.querySelector("#device-label-status");
  const nameTitle = root.querySelector("#device-settings-title");
  const projectsDeviceName = root.querySelector("#device-projects-device-name");
  const deactivate = root.querySelector("#device-deactivate");
  const deactivateStatus = root.querySelector("#device-deactivate-status");
  let active = true;
  let connectionAttempt = 0;
  let session = null;
  let disposePanels = null;
  let disposeUpdates = null;
  const disposeUpdatePanel = () => {
    disposeUpdates?.();
    disposeUpdates = null;
    const panel = root.querySelector("#device-updates-panel");
    if (panel) panel.innerHTML = "";
  };
  let browserOpen = false;
  let savingAttempt = null;
  let savingName = false;
  let deactivating = false;
  let projectsPath = null;
  const settingsRecord = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    if (!active || !settings) return;
    projectsPath = settings.projects_dir;
    pathLabel.textContent = projectsPath || "Unavailable";
    if (session && projectsPath) change.disabled = false;
  });
  const stopWatchingDevices = onDevicesChanged((devices) => {
    if (!active) return;
    const current = devices.find((item) => item.id === deviceId);
    if (!current) {
      active = false;
      connectionAttempt += 1;
      session?.close();
      session = null;
      disposePanels?.();
      disposePanels = null;
      disposeUpdatePanel();
      settingsRecord.dispose();
      stopWatchingDevices();
      showDeactivated(root, onDeviceDeactivated, deviceId);
      return;
    }
    device = current;
    nameTitle.textContent = `${current.name} settings`;
    projectsDeviceName.textContent = current.name;
    if (savingName || nameInput.ownerDocument.activeElement !== nameInput) nameInput.value = current.name;
  });
  const closeBrowser = () => {
    if (browserOpen) $("#scrim").classList.remove("show");
    browserOpen = false;
  };
  registerDispose(() => {
    active = false;
    stopWatchingDevices();
    closeBrowser();
    disposePanels?.();
    disposeUpdatePanel();
    settingsRecord.dispose();
    session?.close();
  });
  nameForm.onsubmit = async (event) => {
    event.preventDefault();
    const name = nameInput.value.trim();
    if (!active || savingName || !name) return;
    savingName = true;
    nameForm.querySelector("button").disabled = true;
    nameStatus.textContent = "Saving…";
    try {
      const renamed = await renameDevice(device.id, name);
      await cacheRenamedDevice(device.id, renamed.name);
      if (!active) return;
      nameStatus.textContent = "Saved.";
    } catch (error) {
      if (!active) return;
      nameInput.value = device.name;
      nameStatus.textContent = error.message;
    } finally {
      if (active) nameForm.querySelector("button").disabled = false;
      savingName = false;
    }
  };
  deactivate.onclick = async () => {
    if (deactivating) return;
    deactivating = true;
    deactivate.disabled = true;
    deactivateStatus.textContent = "";
    const confirmed = await confirmAction({
      title: `Deactivate ${device.name}?`,
      warnings: ["This device must be paired again before it can reconnect."],
      actions: ["Revoke this device's access to your account.", "Close its active connection and remove its local session data."],
      confirmLabel: "Deactivate device",
      danger: true,
    });
    if (!active) return;
    if (!confirmed) {
      deactivating = false;
      deactivate.disabled = false;
      deactivate.focus();
      return;
    }
    deactivate.textContent = "Deactivating…";
    try {
      await revokeDevice(device.id);
      await cacheRevokedDevice(device.id);
      active = false;
      connectionAttempt += 1;
      session?.close();
      session = null;
      disposePanels?.();
      disposePanels = null;
      disposeUpdatePanel();
      settingsRecord.dispose();
      retireDevice(device.id);
    } catch (error) {
      if (!active) return;
      deactivating = false;
      deactivateStatus.textContent = error.message;
      deactivate.textContent = "Deactivate device…";
      deactivate.disabled = false;
    }
  };
  // Nothing here can be read or written without the connection, so a panel that
  // asks after it has gone is refused in the account's own words for a machine
  // that is not there.
  const callRpc = (...asked) => {
    if (!active || !session) return Promise.reject(new Error(deviceOfflineText(device.name)));
    return session.call(...asked);
  };
  // Every panel here is this machine's answer, so none of them exists until the
  // machine is answering: a page that cannot connect says that once, in its
  // status line, rather than standing up six panels that all say it again.
  const standUpPanels = () => {
    disposePanels?.();
    disposeUpdatePanel();
    disposeUpdates = mountBridgeUpdatePanel(root.querySelector("#device-updates-panel"), { deviceId, callRpc });
    disposePanels = standUpDevicePanels({
      projectsHost: root.querySelector("#device-projects-panel"),
      bridgeHost: root.querySelector("#device-bridge-panels"),
      callRpc,
      device,
    });
  };
  const save = async (path) => {
    const attempt = connectionAttempt;
    const owner = session;
    const current = () => active && attempt === connectionAttempt && owner === session;
    if (!active || savingAttempt === attempt) return;
    savingAttempt = attempt;
    try {
      const settings = await callRpc("settings.set", { projects_dir: path });
      if (!current()) return;
      await settingsRecord.write(settings);
      if (!current()) return;
      status.textContent = "Saved. New projects will use this folder.";
      closeBrowser();
    } catch (error) {
      if (current()) {
        const message = $("#berr") || status;
        message.textContent = error.message;
      }
    } finally {
      if (savingAttempt === attempt) savingAttempt = null;
    }
  };
  change.onclick = () => {
    browserOpen = true;
    void openBrowser({
      title: `Choose a projects folder on ${device.name}`,
      deviceId,
      gitOnly: false,
      fallbackFromMissingStart: true,
      startPath: pathLabel.textContent,
      callRpc,
      onChoose: save,
      onCancel: closeBrowser,
    });
  };
  const disconnected = (attempt) => {
    if (!active || attempt !== connectionAttempt) return;
    connectionAttempt += 1;
    session?.close();
    session = null;
    disposePanels?.();
    disposePanels = null;
    disposeUpdatePanel();
    closeBrowser();
    change.disabled = true;
    status.textContent = "Device disconnected. Bring it online, then retry.";
    retry.hidden = false;
  };
  const refreshSettings = (current) => settingsRecord.pull(async () => {
    const settings = await callRpc("settings.get");
    if (!current()) throw new Error("The device connection changed.");
    return settings;
  });
  const connect = async () => {
    const attempt = ++connectionAttempt;
    const current = () => active && attempt === connectionAttempt;
    change.disabled = true;
    retry.hidden = true;
    status.textContent = "Connecting…";
    try {
      session?.close();
      const opened = await openDeviceSettingsSession(device.id, { onLost: () => disconnected(attempt) });
      if (!current()) { opened.close(); return; }
      session = opened;
      change.disabled = !projectsPath;
      standUpPanels();
      await refreshSettings(current);
      if (!current()) return;
      status.textContent = "";
      change.disabled = !projectsPath;
    } catch (error) {
      if (!current()) return;
      session?.close();
      session = null;
      disposePanels?.();
      disposePanels = null;
      disposeUpdatePanel();
      if (!active) return;
      if (!projectsPath) pathLabel.textContent = "Unavailable";
      status.textContent = error.message;
      retry.hidden = false;
    }
  };
  retry.onclick = connect;
  const refusal = whyNothingCanBeAsked(device);
  if (refusal) {
    pathLabel.textContent = refusal.path;
    status.textContent = refusal.words;
    retry.hidden = !refusal.retry;
    return;
  }
  await connect();
}

function showDeactivated(root, onDeviceDeactivated, deviceId) {
  if (onDeviceDeactivated) onDeviceDeactivated(deviceId);
  else root.innerHTML = '<div class="board-head"><h1>Device deactivated</h1></div><p>This device must be paired again before it can reconnect.</p><a class="btn" href="#/account/settings">Local settings</a>';
}

/**
 * Why this machine can be asked nothing at all, before a socket is opened for
 * it — or null, which is the page standing itself up.
 *
 * Two ways a page about a machine has nothing to stand on, and each says one
 * sentence in the status line rather than six panels that all fail. A machine
 * the account calls offline is a wait, so it keeps the way back on. A bridge
 * speaking an API major no adapter here claims is answering, in a shape this
 * tab cannot read: retrying reads the same shape again, so there is no Retry
 * and no folder to choose — the fix is on one side or the other, which is what
 * the notice says.
 */
function whyNothingCanBeAsked(device) {
  if (device.status !== "online")
    return { path: "Unavailable while offline", words: "Bring this device online, then retry to configure it.", retry: true };
  if (contextFor(device.id)?.unsupported)
    return { path: "Unavailable", words: deviceOfflineNotice(device.id), retry: false };
  return null;
}
