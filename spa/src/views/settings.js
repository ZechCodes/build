// Account settings: what belongs to the account rather than to any one machine
// — the privacy story, where creation goes, the defaults a new issue starts
// with, this browser's own preferences, and the devices & keys panel (api-backed,
// so it stays live even when every bridge is offline).
//
// Everything a bridge owns — the projects it holds, the folder it keeps them in,
// how agents run there — is on that machine's own page, which this one links to.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { refreshDevices } from "../devices.js";
import { chooseCreationDevice, retireDevice } from "../connection.js";
import { creationDeviceId } from "../core/devicePolicy.js";
import { fetchDownloads, mintInstallCommand, revokeDevice } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { disablePush, enablePush, pushState } from "../push.js";
import { bindThemeControl, loadThemePreference, themeControlHtml } from "../core/theme.js";
import { loadAgentDefaults, saveAgentDefaults, reconcileAgentDefaults } from "../core/agentDefaults.js";
import { chosenProviderId } from "../core/agentChoice.js";
import { deviceCatalog } from "../core/inboxDevices.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  providerOptionsHtml,
  creatableCatalog,
} from "../core/modelPicker.js";

/** One paired machine: what it is called, the key it holds, whether it is
 *  reachable, the way to its own settings, and the way to unpair it. The link
 *  is minted by the router, like every other link in the app. */
const deviceRowHtml = (device) => `
        <div class="projrow"><span class="pname">${esc(device.name)}</span>
          <span class="ppath mono" style="font-size:11px" title="${esc(device.fingerprint)}">${esc(device.fingerprint.slice(0, 16))}…</span>
          <span class="dim" style="font-size:11.5px"><span class="dot" style="background:${device.status === "online" ? "var(--green)" : "var(--dim)"}"></span> ${esc(device.status)}</span>
          <a class="btn mini devsettings" href="${esc(hashFromRoute({ name: "device", id: device.id }))}">Settings…</a>
          <button class="btn mini revoke" data-id="${esc(device.id)}">Revoke</button></div>`;

/** The paired machines, as the creation choice offers them: the account's own
 *  list, in its own order, with the machine creation goes to today shown. */
function creationDeviceOptionsHtml(devices, chosenId) {
  const options = devices
    .map((device) => `<option value="${esc(device.id)}"${device.id === chosenId ? " selected" : ""}>${esc(device.name)}</option>`)
    .join("");
  return options || '<option value="">No devices yet</option>';
}

/** The one control for home: which machine new projects and captures go to.
 *  Mounted once the account list has been read, since the list is what it
 *  offers. */
function mountCreationDevice() {
  const select = $("#creationdev");
  if (!select) return;
  select.innerHTML = creationDeviceOptionsHtml(App.devices, creationDeviceId(App.devices, App.selectedDeviceId));
  select.disabled = App.devices.length === 0;
  select.onchange = () => chooseCreationDevice(select.value);
}

export async function renderSettings() {
  $("#root").innerHTML = `
    <div class="board-head"><div><h1>Settings</h1><p>Your keys, your custody.</p></div></div>
    <p class="settings-intro" style="margin-top:18px">Build's servers move ciphertext. Every device holds its own key, and only paired devices can read your tasks, plans, and diffs.</p>
    <div class="panel">
      <h3>🔒 What our servers see</h3>
      <div class="row"><span class="k">Routing IDs</span><span class="v">which device a blob is for — random identifiers, no names</span></div>
      <div class="row"><span class="k">Ciphertext sizes</span><span class="v">how big each encrypted blob is</span></div>
      <div class="row"><span class="k">Timing</span><span class="v">when blobs move</span></div>
      <div class="row last"><span class="k">Nothing else</span><span class="v">no goals, no plans, no diffs, no terminal bytes — content decrypts only on your devices</span></div>
    </div>
    <div class="panel">
      <h3>🖥️ Creation device</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">The inbox and the projects rail show every device. This is the one that takes new work.</div>
      <div class="field" style="max-width:340px">
        <label for="creationdev">New projects and captures go to</label>
        <select id="creationdev" disabled><option>loading…</option></select>
      </div>
    </div>
    <div class="panel">
      <h3>🤖 Agent defaults</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">What a new issue starts with. You can still change any of it per issue, under the harness button in the New issue sheet.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:150px"><label>Agent</label><select id="defprovider"><option>loading…</option></select></div>
        <div class="field" style="flex:1;min-width:150px"><label>Model</label><select id="defmodel"><option value="">Harness default</option></select></div>
        <div class="field" style="flex:1;min-width:150px"><label>Reasoning effort</label><select id="defeffort"><option value="">Default effort</option></select></div>
      </div>
      <div class="dim" id="defsaved" style="font-size:12px;min-height:16px"></div>
    </div>
    <div class="panel">
      <h3>🎨 Appearance</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">System follows your OS, and keeps following it — including when it turns dark at dusk.</div>
      ${themeControlHtml(loadThemePreference())}
    </div>
    <div class="panel">
      <h3>🔔 Notifications</h3>
      <div class="dim" style="font-size:13px;margin-bottom:8px">Get a nudge when a task needs you — a plan or diff to review, or an agent that's blocked. Notifications are content-free: they never include your goals, plans, or diffs.</div>
      <div class="addproj">
        <button class="btn" id="pushtoggle" disabled>checking…</button>
        <span class="dim" id="pushstate" style="font-size:13px"></span>
      </div>
      <div class="adderr" id="pusherr"></div>
    </div>
    <div class="panel">
      <h3>⬇️ Downloads</h3>
      <div class="dim" style="font-size:13px;margin-bottom:8px">Install the bridge on another machine, or update this one.</div>
      ${downloadsPlaceholderHtml()}
    </div>
    <div class="panel">
      <h3>📱 Devices &amp; keys</h3>
      <div class="dim" style="font-size:13px;margin-bottom:8px">Only paired devices can read your tasks. When you add one, confirm its fingerprint matches what the bridge printed. Each device's own settings — its projects, its folder, how agents run there — live on its page.</div>
      <div id="devlist"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="addproj"><button class="btn primary" id="adddev">Add a device…</button></div>
      <div class="adderr" id="deverr"></div>
    </div>`;

  await mountAgentDefaults();
  bindThemeControl($("#themepick"));

  // The agent defaults panel: the same three selectors the New issue sheet hides
  // behind its harness button, saved on every change (there is no Save button —
  // a preference with a commit step is a preference people forget to commit).
  async function mountAgentDefaults() {
    const providerSelect = $("#defprovider");
    if (!providerSelect) return;
    const catalog = await deviceCatalog(null);
    let current = loadAgentDefaults();
    const note = $("#defsaved");

    // These defaults are spent creating agents, so they offer what every create
    // surface offers: the two agents, never the carrier behind either family —
    // that question is the machine's, and is asked on its own page.
    const offered = creatableCatalog(catalog);

    const paint = () => {
      providerSelect.innerHTML = providerOptionsHtml(offered.providers, chosenProviderId(offered, current));
      const providerCatalog = catalogForProvider(offered, providerSelect.value);
      $("#defmodel").innerHTML = modelOptionsHtml(providerCatalog.models, modelInCatalog(providerCatalog.models, current.model));
      const supported = effortSupported(providerCatalog.models, $("#defmodel").value);
      $("#defeffort").innerHTML = effortOptionsHtml(providerCatalog.efforts, supported ? current.effort : "", $("#defmodel").value);
      $("#defeffort").disabled = !supported;
    };
    const store = (next, message) => {
      current = saveAgentDefaults(next);
      paint();
      if (note) note.textContent = message;
    };

    paint();
    providerSelect.onchange = () =>
      store(reconcileAgentDefaults({ ...current, provider: providerSelect.value }, { providerChanged: true }), "Saved.");
    $("#defmodel").onchange = () =>
      store(reconcileAgentDefaults({ ...current, model: $("#defmodel").value }, { modelChanged: true }), "Saved.");
    $("#defeffort").onchange = () => store({ ...current, effort: $("#defeffort").value }, "Saved.");
  }

  // Notifications: a single toggle backed by the browser's push subscription.
  const refreshPushToggle = async () => {
    const toggle = $("#pushtoggle");
    const stateLabel = $("#pushstate");
    const state = await pushState();
    toggle.disabled = state === "unsupported" || state === "denied";
    if (state === "unsupported") {
      toggle.textContent = "Not available";
      stateLabel.textContent = "this browser does not support web push";
    } else if (state === "denied") {
      toggle.textContent = "Blocked";
      stateLabel.textContent = "notifications are blocked in your browser settings";
    } else if (state === "enabled") {
      toggle.textContent = "Turn off notifications";
      stateLabel.textContent = "this browser gets a nudge when a task needs you";
    } else {
      toggle.textContent = "Turn on notifications";
      stateLabel.textContent = "off — you'll only see changes when the app is open";
    }
  };
  await refreshPushToggle();
  $("#pushtoggle").onclick = async () => {
    const toggle = $("#pushtoggle");
    toggle.disabled = true;
    $("#pusherr").textContent = "";
    try {
      const state = await pushState();
      if (state === "enabled") await disablePush();
      else await enablePush();
    } catch (e) {
      $("#pusherr").textContent = e.message;
    }
    await refreshPushToggle();
  };

  // Devices: list the user's paired devices (over plain HTTP, not the bridge),
  // each with its fingerprint, online/offline, and a revoke action.
  const refreshDeviceList = async () => {
    try {
      const devices = await refreshDevices();
      $("#devlist").innerHTML = devices.length
        ? devices.map(deviceRowHtml).join("")
        : '<div class="dim" style="font-size:13px">No devices yet. Install the bridge above, then add it with its pairing code.</div>';
      $("#devlist").querySelectorAll(".revoke").forEach(
        (btn) =>
          (btn.onclick = async () => {
            btn.disabled = true;
            $("#deverr").textContent = "";
            try {
              await revokeDevice(btn.dataset.id);
              // A device the account no longer has cannot be asked anything:
              // its drafts, its cached reads, its link and its session go with
              // it, and every surface over it is told.
              retireDevice(btn.dataset.id);
              await refreshDeviceList();
            } catch (e) {
              $("#deverr").textContent = e.message;
              btn.disabled = false;
            }
          }),
      );
    } catch (e) {
      $("#devlist").innerHTML = `<div class="adderr">${esc(e.message)}</div>`;
    }
  };
  await refreshDeviceList();
  $("#adddev").onclick = () => openAddDevice(refreshDeviceList);
  mountCreationDevice();

  // The same block the first-run gate mounts — one renderer, two hosts. It is
  // the only thing here that asks the api rather than the bridge, and nothing
  // on the page depends on its answer, so it goes last and is not awaited: the
  // page is done when the bridge-side mounts are. It paints itself into the
  // placeholder when the api answers, and names its own refusal in
  // #downloadserr, so a slow round trip leaves a "loading…" line — not a page
  // of dead buttons, and not a caller (renderAccount, which mounts the account
  // nav next) waiting on the api's clock.
  void mountDownloads($("#root"), {
    fetchDownloads,
    mintInstallCommand,
    platformKey: currentPlatformKey(),
    clipboard: navigator.clipboard,
  });
}
