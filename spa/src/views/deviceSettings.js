// One machine's settings: everything the bridge on it owns — its projects,
// where they are kept, and how agents run there.
//
// The page keeps its own transport so browsing another machine cannot redirect
// workspace requests, or a late folder selection, to a different host. That is
// also why every panel here is handed this page's caller: the account page holds
// what is the account's, and every answer on this one belongs to this machine.
import { $ } from "../dom.js";
import { deviceOfflineText, esc } from "../core/text.js";
import { deviceOfflineNotice } from "../core/deviceNotice.js";
import { contextFor } from "../core/deviceContexts.js";
import { App } from "../app.js";
import { openDeviceSettingsSession } from "../connection.js";
import { openBrowser } from "../sheets/browser.js";
import { standUpDevicePanels } from "./devicePanels.js";

export async function renderDeviceSettings({ root = $("#root"), deviceId = App.route.id, embedded = false, registerDispose = (dispose) => { App.viewDispose = dispose; } } = {}) {
  const device = App.devices.find((item) => item.id === deviceId);
  root.classList.add("device-settings");
  if (!device) {
    root.innerHTML = '<div class="board-head"><h1>Device not found</h1></div><p>This device is no longer paired with your account.</p><a class="btn" href="#/account/settings">Account settings</a>';
    return;
  }
  root.innerHTML = `
    ${embedded ? "" : '<a class="btn mini" href="#/account/settings">Local settings</a>'}
    <div class="board-head"><div><h1>${esc(device.name)} settings</h1><p>The projects this machine holds, and how agents run on it.</p></div></div>
    <div id="device-projects-panel"></div>
    <div class="panel">
      <h3>Projects folder</h3>
      <p class="dim">New projects and cloned repositories will be kept in this folder on ${esc(device.name)}. Existing projects stay where they are.</p>
      <div class="projfolder"><code id="device-projects-path">Loading…</code>
        <button class="btn" id="device-projects-change" disabled>Choose folder…</button></div>
      <p id="device-settings-status" role="status" aria-live="polite"></p>
      <button class="btn mini" id="device-settings-retry" hidden>Retry</button>
    </div>
    <div id="device-bridge-panels"></div>`;
  const pathLabel = root.querySelector("#device-projects-path");
  const change = root.querySelector("#device-projects-change");
  const status = root.querySelector("#device-settings-status");
  const retry = root.querySelector("#device-settings-retry");
  let active = true;
  let connectionAttempt = 0;
  let session = null;
  let browserOpen = false;
  let savingAttempt = null;
  const closeBrowser = () => {
    if (browserOpen) $("#scrim").classList.remove("show");
    browserOpen = false;
  };
  registerDispose(() => {
    active = false;
    closeBrowser();
    session?.close();
  });
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
  const standUpPanels = () =>
    standUpDevicePanels({
      projectsHost: root.querySelector("#device-projects-panel"),
      bridgeHost: root.querySelector("#device-bridge-panels"),
      callRpc,
      device,
    });
  const save = async (path) => {
    const attempt = connectionAttempt;
    const owner = session;
    const current = () => active && attempt === connectionAttempt && owner === session;
    if (!active || savingAttempt === attempt) return;
    savingAttempt = attempt;
    try {
      const settings = await callRpc("settings.set", { projects_dir: path });
      if (!current()) return;
      pathLabel.textContent = settings.projects_dir;
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
    closeBrowser();
    change.disabled = true;
    status.textContent = "Device disconnected. Bring it online, then retry.";
    retry.hidden = false;
  };
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
      const settings = await callRpc("settings.get");
      if (!current()) return;
      pathLabel.textContent = settings.projects_dir;
      status.textContent = "";
      change.disabled = false;
      await standUpPanels();
    } catch (error) {
      if (!current()) return;
      session?.close();
      session = null;
      if (!active) return;
      pathLabel.textContent = "Unavailable";
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
