// Settings: projects (which repo agents work on), the privacy story, and the
// devices & keys panel (api-backed, stays live even when the bridge is offline).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { refreshDevices } from "../devices.js";
import { fetchDownloads, revokeDevice } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openBrowser } from "../sheets/browser.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { openSetRemote } from "../sheets/setRemote.js";
import { openClone } from "../sheets/clone.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { disablePush, enablePush, pushState } from "../push.js";
import { bindThemeControl, loadThemePreference, themeControlHtml } from "../core/theme.js";
import { loadAgentDefaults, saveAgentDefaults, reconcileAgentDefaults } from "../core/agentDefaults.js";
import { chosenProviderId } from "../core/agentChoice.js";
import { defaultHarnessPanelHtml, mountDefaultHarness } from "../core/defaultHarness.js";
import { loadModelCatalog } from "../app.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  providerOptionsHtml,
  creatableCatalog,
} from "../core/modelPicker.js";

export async function renderSettings() {
  $("#root").innerHTML = `
    <div class="board-head"><div><h1>Settings</h1><p>Your keys, your custody.</p></div></div>
    <div class="panel">
      <h3>📁 Projects</h3>
      <div id="projlist"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="addproj">
        <button class="btn primary" id="newrepo">New repo…</button>
        <button class="btn" id="browseadd">Browse for a repo…</button>
        <button class="btn" id="cloneadd">Clone from URL…</button>
      </div>
      <div class="projfolder">Projects folder: <code id="pdir">…</code>
        <button class="btn mini" id="changedir">Change…</button>
        <span class="dim">clones land here</span></div>
      <details class="manualadd"><summary>or enter a path manually</summary>
        <div class="addproj"><input id="projpath" class="path" placeholder="~/code/your-repo" />
          <input id="projbranch" placeholder="auto" style="max-width:90px" />
          <button class="btn" id="addproj">Add</button></div></details>
      <div class="adderr" id="adderr"></div>
    </div>
    <p class="settings-intro" style="margin-top:18px">Build's servers move ciphertext. Every device holds its own key, and only paired devices can read your tasks, plans, and diffs.</p>
    <div class="panel">
      <h3>🔒 What our servers see</h3>
      <div class="row"><span class="k">Routing IDs</span><span class="v">which device a blob is for — random identifiers, no names</span></div>
      <div class="row"><span class="k">Ciphertext sizes</span><span class="v">how big each encrypted blob is</span></div>
      <div class="row"><span class="k">Timing</span><span class="v">when blobs move</span></div>
      <div class="row last"><span class="k">Nothing else</span><span class="v">no goals, no plans, no diffs, no terminal bytes — content decrypts only on your devices</span></div>
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
    ${defaultHarnessPanelHtml()}
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
      <div class="dim" style="font-size:13px;margin-bottom:8px">Only paired devices can read your tasks. When you add one, confirm its fingerprint matches what the bridge printed.</div>
      <div id="devlist"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="addproj"><button class="btn primary" id="adddev">Add a device…</button></div>
      <div class="adderr" id="deverr"></div>
    </div>`;

  const refresh = async () => {
    try {
      const { projects } = await App.call("project.list");
      $("#projlist").innerHTML =
        projects
          .map(
            (p) => `
        <div class="projrow"><span class="pname">${esc(p.name)}</span>
          <span class="ppath">${esc(p.path)}</span><span class="dim" style="font-size:11.5px">${esc(p.base_branch)}</span>
          <span class="premote">${p.remote ? "⇄ " + esc(p.remote) : '<span class="dim">no remote</span>'}</span>
          <button class="btn mini setremote" data-id="${esc(p.project_id)}">Set remote…</button></div>`,
          )
          .join("") || '<div class="dim" style="font-size:13px">No projects yet.</div>';
      const { projects_dir } = await App.call("settings.get");
      $("#pdir").textContent = projects_dir;
      $("#projlist").querySelectorAll(".setremote").forEach(
        (btn) =>
          (btn.onclick = () => {
            const project = projects.find((p) => p.project_id === btn.dataset.id);
            openSetRemote(project, refresh);
          }),
      );
    } catch (e) {
      $("#projlist").innerHTML = `<div class="adderr">${esc(e.message)}</div>`;
    }
  };
  await refresh();
  await mountDefaultHarness($("#root"), { callRpc: (method, params) => App.call(method, params) });
  await mountAgentDefaults();
  bindThemeControl($("#themepick"));
  $("#newrepo").onclick = () => openNewRepo(refresh);

  // The agent defaults panel: the same three selectors the New issue sheet hides
  // behind its harness button, saved on every change (there is no Save button —
  // a preference with a commit step is a preference people forget to commit).
  async function mountAgentDefaults() {
    const providerSelect = $("#defprovider");
    if (!providerSelect) return;
    let catalog = { default_provider: "claude", providers: [] };
    try {
      catalog = await loadModelCatalog();
    } catch {
      providerSelect.innerHTML = '<option value="">(agent catalog unavailable — is your device online?)</option>';
      return;
    }
    let current = loadAgentDefaults();
    const note = $("#defsaved");

    // These defaults are spent creating agents, so they offer what every create
    // surface offers: the two agents, never the carrier behind Claude Code —
    // that question belongs to the Default agent panel below.
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

  // Browse the host filesystem and add the chosen git repo — no typing.
  $("#browseadd").onclick = () =>
    openBrowser({
      title: "Browse for a git repo",
      gitOnly: true,
      onChoose: async (path) => {
        try {
          await App.call("project.add", { path });
          $("#scrim").classList.remove("show");
          await refresh();
        } catch (e) {
          const err = $("#berr");
          if (err) err.textContent = e.message;
        }
      },
    });
  // Clone a remote into the projects folder.
  $("#cloneadd").onclick = () => openClone(refresh);
  // Pick a different projects folder (any directory).
  $("#changedir").onclick = () =>
    openBrowser({
      title: "Choose a projects folder",
      gitOnly: false,
      onChoose: async (path) => {
        try {
          await App.call("settings.set", { projects_dir: path });
          $("#scrim").classList.remove("show");
          await refresh();
        } catch (e) {
          const err = $("#berr");
          if (err) err.textContent = e.message;
        }
      },
    });
  $("#addproj").onclick = async () => {
    const path = $("#projpath").value.trim();
    if (!path) return;
    const base_branch = $("#projbranch").value.trim() || undefined;
    $("#addproj").disabled = true;
    $("#adderr").textContent = "";
    try {
      await App.call("project.add", { path, base_branch });
      $("#projpath").value = "";
      $("#projbranch").value = "";
      await refresh();
    } catch (e) {
      $("#adderr").textContent = e.message;
    }
    $("#addproj").disabled = false;
  };

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
        ? devices
            .map(
              (d) => `
        <div class="projrow"><span class="pname">${esc(d.name)}</span>
          <span class="ppath mono" style="font-size:11px" title="${esc(d.fingerprint)}">${esc(d.fingerprint.slice(0, 16))}…</span>
          <span class="dim" style="font-size:11.5px"><span class="dot" style="background:${d.status === "online" ? "var(--green)" : "var(--dim)"}"></span> ${esc(d.status)}</span>
          <button class="btn mini revoke" data-id="${esc(d.id)}">Revoke</button></div>`,
            )
            .join("")
        : '<div class="dim" style="font-size:13px">No devices yet. Install the bridge above, then add it with its pairing code.</div>';
      $("#devlist").querySelectorAll(".revoke").forEach(
        (btn) =>
          (btn.onclick = async () => {
            btn.disabled = true;
            $("#deverr").textContent = "";
            try {
              await revokeDevice(btn.dataset.id);
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
    platformKey: currentPlatformKey(),
    clipboard: navigator.clipboard,
  });
}
