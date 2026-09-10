// Device settings keep their own transport so browsing another machine cannot
// redirect workspace requests, or a late folder selection, to a different host.
import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { openDeviceSettingsSession } from "../connection.js";
import { openBrowser } from "../sheets/browser.js";

export async function renderDeviceSettings() {
  const device = App.devices.find((item) => item.id === App.route.id);
  const root = $("#root");
  root.classList.add("device-settings");
  if (!device) {
    root.innerHTML = '<div class="board-head"><h1>Device not found</h1></div><p>This device is no longer paired with your account.</p><a class="btn" href="#/account/settings">Account settings</a>';
    return;
  }
  root.innerHTML = `
    <a class="btn mini" href="#/account/settings">Account settings</a>
    <div class="board-head"><div><h1>${esc(device.name)} settings</h1><p>Settings for this device.</p></div></div>
    <div class="panel">
      <h3>Projects folder</h3>
      <p class="dim">New projects and cloned repositories will be kept in this folder on ${esc(device.name)}. Existing projects stay where they are.</p>
      <div class="projfolder"><code id="device-projects-path">Loading…</code>
        <button class="btn" id="device-projects-change" disabled>Choose folder…</button></div>
      <p id="device-settings-status" role="status" aria-live="polite"></p>
      <button class="btn mini" id="device-settings-retry" hidden>Retry</button>
    </div>`;
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
  App.viewDispose = () => {
    active = false;
    closeBrowser();
    session?.close();
  };
  const callRpc = (method, params) => {
    if (!active || !session) return Promise.reject(new Error("Device settings are no longer open."));
    return session.call(method, params);
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
  if (device.status !== "online") {
    pathLabel.textContent = "Unavailable while offline";
    status.textContent = "Bring this device online, then retry to choose its projects folder.";
    retry.hidden = false;
    return;
  }
  await connect();
}
