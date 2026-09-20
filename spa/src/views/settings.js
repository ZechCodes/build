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
import { deviceNameOf, homeDeviceId } from "../core/devicePolicy.js";
import { fetchDownloads, mintInstallCommand, revokeDevice } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { disablePush, enablePush, pushState } from "../push.js";
import { bindThemeControl, loadThemePreference, themeControlHtml } from "../core/theme.js";
import { harnessDefaultsPanelHtml, mountHarnessDefaults } from "../core/harnessDefaults.js";
import { deviceCatalog } from "../core/inboxDevices.js";
import { onDeviceStateChanged } from "../core/deviceContexts.js";
import { clearConnectionDiagnosticHistory, connectionDiagnosticHistory } from "../core/connectionDiagnostics.js";
import { connectionDiagnosticsPanelHtml, mountConnectionDiagnostics } from "../core/connectionDiagnosticsPanel.js";
import { buildVersionLineHtml, mountBuildVersionLine } from "../core/buildVersionLine.js";

/** The sha this bundle was built at, as core/version.js and core/changeEvents.js
 *  read it. `dev` for a bundle CI never stamped, which is what a dev server
 *  serves and what the suites see. */
const BUNDLE_VERSION = import.meta.env.VITE_BUILD_VERSION || "dev";

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

/** The machine the control shows: the one the account picked, while the account
 *  still has it. A picked machine that is merely away is still the machine new
 *  work is meant for; only a pick the list no longer carries falls back to
 *  whoever is taking the work today. */
function shownCreationDevice(devices, pickedId) {
  const picked = devices.find((device) => device.id === pickedId);
  return picked?.id || homeDeviceId(devices, pickedId);
}

/** Where new work is going while the machine the account picked is away.
 *  Nothing to say while that machine is the one taking the work; said plainly
 *  when it is not, because a control that names one machine and means another
 *  is a control nobody can act on. */
function creationFallbackNote(devices, shownId) {
  const shown = devices.find((device) => device.id === shownId);
  if (!shown || shown.status === "online") return "";
  const landing = deviceNameOf(devices, homeDeviceId(devices, shownId));
  return landing
    ? `${shown.name} is offline; new work goes to ${landing} until it returns.`
    : `${shown.name} is offline; new work waits until a device is back.`;
}

/**
 * The one control for home: which machine new projects and captures go to.
 *
 * Mounted once the account list has been read, since the list is what it
 * offers — and painted again whenever what it says could have changed. Both
 * halves are one function of (the account list, the pick), so the note can
 * never be about a machine the select is not showing: picking another machine
 * repaints from the new pick, and a machine going or coming back repaints from
 * the list, the way every other surface that greys what a lost machine holds
 * is told (core/deviceContexts.js). The listener is this page's teardown.
 */
function mountCreationDevice(root, registerDispose) {
  const $ = (selector) => root.querySelector(selector);
  const select = $("#creationdev");
  if (!select) return;
  let offered = null; // the options last painted, so a repaint that says the
  // same thing never rebuilds a list the reader may have open
  const paint = (pickedId) => {
    const shownId = shownCreationDevice(App.devices, pickedId);
    const options = creationDeviceOptionsHtml(App.devices, shownId);
    if (options !== offered) {
      offered = options;
      select.innerHTML = options;
    }
    select.disabled = App.devices.length === 0;
    const note = $("#creationfallback");
    if (note) note.textContent = creationFallbackNote(App.devices, shownId);
  };
  paint(App.selectedDeviceId);
  select.onchange = () => {
    chooseCreationDevice(select.value);
    paint(select.value);
  };
  registerDispose(onDeviceStateChanged(() => paint(App.selectedDeviceId)));
}

export async function renderSettings({ root = $("#root"), registerDispose = (dispose) => { App.viewDispose = dispose; }, isCurrent = () => true, onDevicesChanged = () => {} } = {}) {
  const $ = (selector) => root.querySelector(selector);
  let disposeCreation = null;
  let disposePairing = null;
  let disposeDiagnostics = null;
  registerDispose(() => { disposeCreation?.(); disposePairing?.(); disposeDiagnostics?.(); });
  root.innerHTML = `
    <div class="board-head"><div><h1>Local settings</h1><p>Preferences saved in this browser.</p></div></div>
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
        <div class="dim" id="creationfallback" style="font-size:12.5px;margin-top:6px" role="status"></div>
      </div>
    </div>
    ${harnessDefaultsPanelHtml()}
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
    ${connectionDiagnosticsPanelHtml()}
    ${buildVersionLineHtml(BUNDLE_VERSION)}
    <div class="panel">
      <h3>📱 Devices &amp; keys</h3>
      <div class="dim" style="font-size:13px;margin-bottom:8px">Only paired devices can read your tasks. When you add one, confirm its fingerprint matches what the bridge printed. Each device's own settings — its projects, its folder, how agents run there — live on its page.</div>
      <div id="devlist"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="addproj"><button class="btn primary" id="adddev">Add a device…</button></div>
      <div class="adderr" id="deverr"></div>
    </div>`;

  await mountAgentDefaults();
  if (!isCurrent()) return;
  bindThemeControl($("#themepick"));

  // The agent defaults panel: a model and effort per harness, and the harness
  // new work starts on, read from the creation device's own catalog and saved
  // on every change. What a PROJECT agent starts on is not here — that one is
  // the machine's, asked on its own page (core/projectAgentSetting.js).
  async function mountAgentDefaults() {
    if (!$("#defprovider")) return;
    const catalog = await deviceCatalog(null);
    if (!isCurrent()) return;
    mountHarnessDefaults(root, { catalog });
  }

  // Notifications: a single toggle backed by the browser's push subscription.
  const refreshPushToggle = async () => {
    const toggle = $("#pushtoggle");
    const stateLabel = $("#pushstate");
    const state = await pushState();
    if (!isCurrent()) return;
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
  if (!isCurrent()) return;
  $("#pushtoggle").onclick = async () => {
    const toggle = $("#pushtoggle");
    toggle.disabled = true;
    $("#pusherr").textContent = "";
    try {
      const state = await pushState();
      if (!isCurrent()) return;
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
      if (!isCurrent()) return;
      onDevicesChanged();
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
  if (!isCurrent()) return;
  $("#adddev").onclick = () => { disposePairing = openAddDevice(refreshDeviceList); };
  mountCreationDevice(root, (dispose) => { disposeCreation = dispose; });

  // The connection dump. It reads the history through the module rather than
  // the `buildConnectionDiagnostics` global, and the machines through the
  // account list, so a device that has a name is named. Its poll is this page's
  // teardown — nothing ticks once Settings is off screen.
  mountBuildVersionLine(root, { version: BUNDLE_VERSION, clipboard: navigator.clipboard });

  disposeDiagnostics = mountConnectionDiagnostics(root, {
    history: connectionDiagnosticHistory,
    clear: clearConnectionDiagnosticHistory,
    devices: () => App.devices,
  });

  // The same block the first-run gate mounts — one renderer, two hosts. It is
  // the only thing here that asks the api rather than the bridge, and nothing
  // on the page depends on its answer, so it goes last and is not awaited: the
  // page is done when the bridge-side mounts are. It paints itself into the
  // placeholder when the api answers, and names its own refusal in
  // #downloadserr, so a slow round trip leaves a "loading…" line — not a page
  // of dead buttons, and not the modal that hosts this panel waiting on the
  // api's clock.
  void mountDownloads(root, {
    fetchDownloads,
    mintInstallCommand,
    platformKey: currentPlatformKey(),
    clipboard: navigator.clipboard,
  });
}
