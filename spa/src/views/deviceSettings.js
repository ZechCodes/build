// One machine's settings: everything the bridge on it owns — its projects,
// where they are kept, and how agents run there.
//
// The page keeps its own transport so browsing another machine cannot redirect
// workspace requests, or a late folder selection, to a different host. That is
// also why every panel here is handed this page's caller: the account page
// holds what is the account's, and each of these answers belong to one machine.
import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { openDeviceSettingsSession } from "../connection.js";
import { contextFor } from "../core/deviceContexts.js";
import { refreshFeed } from "../core/taskFeed.js";
import { openBrowser } from "../sheets/browser.js";
import { deviceProjectsPanelHtml, mountDeviceProjects } from "./deviceProjects.js";
import { agentModesPanelHtml, mountAgentModes } from "../core/agentModes.js";
import { defaultHarnessPanelHtml, mountDefaultHarness } from "../core/defaultHarness.js";
import { ACCOUNT_ISOLATION, isolationPanelHtml, mountIsolation } from "../core/isolation.js";
import { mountTriageSetting, triageSettingPanelHtml } from "../core/triageSetting.js";

/** The panels this machine's bridge owns: each states its own markup and mounts
 *  itself on this page's connection, so the page stands them up in one pass and
 *  adding another is a line here. */
const BRIDGE_PANELS = [
  { html: agentModesPanelHtml, mount: (host, options) => mountAgentModes(host, options) },
  { html: defaultHarnessPanelHtml, mount: (host, options) => mountDefaultHarness(host, options) },
  { html: isolationPanelHtml, mount: (host, options) => mountIsolation(host, { ...options, target: ACCOUNT_ISOLATION }) },
  { html: triageSettingPanelHtml, mount: (host, options) => mountTriageSetting(host, options) },
];

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
    ${deviceProjectsPanelHtml()}
    <div class="panel">
      <h3>Projects folder</h3>
      <p class="dim">New projects and cloned repositories will be kept in this folder on ${esc(device.name)}. Existing projects stay where they are.</p>
      <div class="projfolder"><code id="device-projects-path">Loading…</code>
        <button class="btn" id="device-projects-change" disabled>Choose folder…</button></div>
      <p id="device-settings-status" role="status" aria-live="polite"></p>
      <button class="btn mini" id="device-settings-retry" hidden>Retry</button>
    </div>
    ${BRIDGE_PANELS.map((panel) => panel.html()).join("")}`;
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
  const callRpc = (...asked) => {
    if (!active || !session) return Promise.reject(new Error("Device settings are no longer open."));
    return session.call(...asked);
  };
  // The account's copy of what this machine offers is what these panels just
  // changed, so the registry is told to ask again. Guarded twice over: this page
  // opens its own connection and may be looking at a machine the app holds no
  // context for, and a refused re-read is the catalog's business, not this
  // save's — the panel has its confirmation either way.
  const refreshAccountCatalog = async () => {
    try {
      await contextFor(device.id)?.refreshModelCatalog();
    } catch {
      // The next surface that asks this machine for its harnesses reads it again.
    }
  };
  const mountBridgePanels = async () => {
    const options = { callRpc, onSaved: refreshAccountCatalog };
    for (const panel of BRIDGE_PANELS) await panel.mount(root, options);
  };
  const readProjects = mountDeviceProjects(root, {
    callRpc,
    deviceName: device.name,
    onProjectCreated: () => refreshFeed(),
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
      await readProjects();
      await mountBridgePanels();
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
