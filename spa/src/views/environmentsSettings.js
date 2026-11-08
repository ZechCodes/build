// M1 settings observes environments and their project boards on every device.
// Cache records own paint; pulls merely refresh those records.
import { App } from "../app.js";
import { onDevicesChanged, readCachedDevices } from "../devices.js";
import { deviceCall } from "../core/inboxDevices.js";
import { bridgeCapabilities } from "../core/changeEvents.js";
import { environmentSupportAddress } from "../core/environmentSupport.js";
import { admitEnvironmentListWrite, environmentsAddress, writeCachedEnvironments } from "../core/environmentCache.js";
import { watchSettingsRecord } from "../core/settingsRecords.js";
import { pullDeviceProjection } from "../core/scopedDeviceReads.js";
import { taskCountText } from "../core/taskCountText.js";
import { esc } from "../core/text.js";
import { openProjectSettings } from "../sheets/projectSettings.js";

const projectsAddress = deviceId => ({ deviceId, entityId: "", kind: "projects" });
const kindGlyph = kind => kind === "environment" ? "▣" : "◇";
const glyphHtml = kind => `<span class="create-kind" aria-hidden="true">${kindGlyph(kind)}</span>`;
const workspacesOf = project => project.workspaces || 0;

function projectHtml(project) {
  const folders = project.added_sources?.length || 0;
  return `<div class="projrow"><span class="pname">${glyphHtml("project")} ${esc(project.name)}</span>
    <span class="dim">+${folders} folders</span>
    <span class="dim">${workspacesOf(project)} workspaces · ${esc(taskCountText(project.open_tasks || 0, project.total_tasks || 0))}</span>
    <button class="btn mini projsettings" data-environment-project="${esc(project.project_id)}">Settings…</button></div>`;
}

function environmentHtml(environment, projects, device) {
  const members = projects.filter(project => project.environment_id === environment.environment_id);
  const workspaces = members.reduce((total, project) => total + workspacesOf(project), 0);
  return `<article class="panel" data-environment="${esc(environment.environment_id)}">
    <h3>${glyphHtml("environment")} ${esc(environment.name)} <small class="dim">${esc(device.name)}</small></h3>
    <p class="ppath">${esc(environment.home_path)}</p>
    <p class="dim">${environment.sources?.length || 0} sources · ${members.length} projects · ${workspaces} workspaces</p>
    ${members.map(projectHtml).join("") || '<p class="dim">no projects yet</p>'}</article>`;
}

function deviceListHtml({ support, environments, projects, error }, device) {
  const rows = support?.list ? environments : projects;
  if (!Array.isArray(rows)) return `<p class="${error ? "adderr" : "dim"}">${esc(error || "loading…")}</p>`;
  if (!support?.list) return `<div class="panel"><h3>Projects (this device needs an update to use environments)</h3>
    ${projects.map(projectHtml).join("") || '<p class="dim">No projects yet.</p>'}</div>`;
  return environments.map(environment => environmentHtml(environment, projects || [], device)).join("")
    || '<p class="dim">No environments on this device yet</p>';
}

async function mountDeviceEnvironments(host, device, active) {
  const callRpc = deviceCall(device.id);
  const state = {};
  const paint = () => {
    if (!active()) return;
    host.innerHTML = `<h2>${esc(device.name)}</h2>${deviceListHtml(state, device)}`;
    host.querySelectorAll("[data-environment-project]").forEach(button => {
      button.onclick = () => openProjectSettings(button.dataset.environmentProject, { callRpc, deviceId: device.id, onDeleted: refresh });
    });
  };
  const entries = [["support", environmentSupportAddress(device.id)], ["environments", environmentsAddress(device.id)], ["projects", projectsAddress(device.id)]];
  const records = Object.fromEntries(entries.map(([kind, address]) => [kind, watchSettingsRecord(address, value => {
    const wasSupported = state.support?.list;
    state[kind] = value;
    paint();
    if (kind === "support" && value?.list && wasSupported === false) void refresh();
  })]));
  const supported = () => active() && bridgeCapabilities(device.id).environments?.list === true;
  async function refreshEnvironments() {
    if (!supported()) return;
    const admission = await admitEnvironmentListWrite(device.id, supported);
    if (!admission || !supported()) return;
    const answer = await callRpc("environment.list");
    if (Array.isArray(answer?.environments)) await writeCachedEnvironments(device.id, answer.environments, admission, supported);
  }
  async function refresh() {
    const reportError = error => { state.error = error.message; paint(); };
    await Promise.allSettled([
      pullDeviceProjection(device.id, records.projects, async () => (await callRpc("project.list")).projects).catch(reportError),
      refreshEnvironments().catch(reportError),
    ]);
    await Promise.all(Object.values(records).map(record => record.read()));
    paint();
  }
  await Promise.all(Object.values(records).map(record => record.whenPainted()));
  void refresh();
  return Object.assign(() => Object.values(records).forEach(record => record.dispose()), {
    update: next => { device = next; paint(); },
  });
}

export async function renderEnvironmentsSettings({ root = document.querySelector("#root"), registerDispose = dispose => { App.viewDispose = dispose; } } = {}) {
  let active = true;
  const mounts = new Map();
  root.innerHTML = '<div class="board-head"><h1>Environments</h1></div><div data-environments-list></div>';
  const list = root.querySelector("[data-environments-list]");
  const pruneDevices = () => {
    for (const [id, mount] of mounts) {
      if (App.devices.some(device => device.id === id)) continue;
      mounts.delete(id); mount.dispose?.(); mount.host.remove();
    }
    if (!mounts.size) list.replaceChildren();
  };
  const mountDevice = async device => {
    const held = mounts.get(device.id);
    if (held) { held.device = device; held.dispose?.update(device); return; }
    const host = document.createElement("section");
    host.dataset.environmentsDevice = device.id;
    list.append(host);
    const mount = { host, device };
    mounts.set(device.id, mount);
    const dispose = await mountDeviceEnvironments(host, device, () => active && mounts.get(device.id) === mount);
    if (active && mounts.get(device.id) === mount) { mount.dispose = dispose; dispose.update(mount.device); }
    else dispose();
  };
  const update = async () => {
    if (!active) return;
    pruneDevices();
    await Promise.all(App.devices.map(mountDevice));
    if (active && !App.devices.length) list.textContent = "No paired devices";
  };
  const unsubscribe = onDevicesChanged(() => { void update(); });
  registerDispose(() => { active = false; unsubscribe(); for (const mount of mounts.values()) mount.dispose?.(); });
  await readCachedDevices();
  await update();
}
