// M1 settings observes environments and their project boards on every device.
// Cache records own paint; pulls merely refresh those records.
import { App } from "../app.js";
import { onDevicesChanged, readCachedDevices } from "../devices.js";
import { deviceCall } from "../core/inboxDevices.js";
import { bridgeCapabilities } from "../core/changeEvents.js";
import { environmentSupportAddress } from "../core/environmentSupport.js";
import { admitEnvironmentListWrite, environmentsAddress, writeCachedEnvironments } from "../core/environmentCache.js";
import { watchSettingsRecord } from "../core/settingsRecords.js";
import { watchEnvironmentSettingsProjects } from "../core/environmentSettingsProjects.js";
import { identityDeviceLabel, kindGlyphHtml } from "../core/environmentIdentity.js";
import { taskCountText } from "../core/taskCountText.js";
import { esc } from "../core/text.js";
import { openProjectSettings } from "../sheets/projectSettings.js";

const projectsAddress = deviceId => ({ deviceId, entityId: "", kind: "projects" });
const knownCount = value => Number.isSafeInteger(value) && value >= 0;

function projectCountText(project) {
  const counts = [];
  if (knownCount(project.workspaces)) counts.push(`${project.workspaces} workspaces`);
  if (knownCount(project.open_tasks) && project.open_tasks > 0) counts.push(taskCountText(project.open_tasks, project.total_tasks));
  else if (knownCount(project.total_tasks)) counts.push(taskCountText(0, project.total_tasks));
  return counts.join(" · ");
}

function environmentWorkspaceText(projects) {
  const counts = projects.map(project => project.workspaces).filter(knownCount);
  return counts.length ? `${counts.reduce((total, count) => total + count, 0)} workspaces` : "";
}

function deviceTagHtml(device) {
  const label = identityDeviceLabel({ deviceId: device.id }, App.devices);
  return label ? `<span class="dim inbox-device">${esc(label)}</span>` : "";
}

function projectHtml(project, device, countsKnown) {
  const folders = countsKnown && Array.isArray(project.added_sources) ? `+${project.added_sources.length} folders` : "";
  return `<div class="projrow"><span class="pname">${kindGlyphHtml("project")} ${esc(project.name)} ${deviceTagHtml(device)}</span>
    <span class="dim">${folders}</span>
    <span class="dim">${countsKnown ? esc(projectCountText(project)) : ""}</span>
    <button class="btn mini projsettings" data-environment-project="${esc(project.project_id)}">Settings…</button></div>`;
}

/** Count fields are authoritative only in the settings-owned project.list.
 * Journal replicas may retain legacy enrichment, so fallback rows are identity. */
function projectListing(settingsProjects, journalProjects) {
  if (Array.isArray(settingsProjects)) return { rows: settingsProjects, countsKnown: true };
  return { rows: Array.isArray(journalProjects) ? journalProjects : null, countsKnown: false };
}

function listStatusHtml(error) {
  return `<p class="${error ? "adderr" : "dim"}">${esc(error || "loading…")}</p>`;
}

function environmentTotalsText(environment, members, countsKnown) {
  const parts = [];
  if (Array.isArray(environment.sources)) parts.push(`${environment.sources.length} sources`);
  if (members) parts.push(`${members.length} projects`);
  const workspaceText = countsKnown ? environmentWorkspaceText(members) : "";
  if (workspaceText) parts.push(workspaceText);
  return parts.join(" · ");
}

function environmentProjectsHtml(members, listing, device, error) {
  if (!members) return listStatusHtml(error);
  return members.map(project => projectHtml(project, device, listing.countsKnown)).join("")
    || '<p class="dim">no projects yet</p>';
}

function environmentHtml(environment, listing, device, error) {
  const members = listing.rows?.filter(project => project.environment_id === environment.environment_id);
  return `<article class="panel" data-environment="${esc(environment.environment_id)}">
    <h3>${kindGlyphHtml("environment")} ${esc(environment.name)} ${deviceTagHtml(device)}</h3>
    <p class="ppath">${esc(environment.home_path)}</p>
    <p class="dim">${environmentTotalsText(environment, members, listing.countsKnown)}</p>
    ${environmentProjectsHtml(members, listing, device, error)}</article>`;
}

function legacyProjectsHtml(listing, device) {
  return `<div class="panel"><h3>Projects (this device needs an update to use environments)</h3>
    ${listing.rows.map(project => projectHtml(project, device, listing.countsKnown)).join("") || '<p class="dim">No projects yet.</p>'}</div>`;
}

function deviceListHtml({ support, environments, projects: settingsProjects, journalProjects, error }, device) {
  const listing = projectListing(settingsProjects, journalProjects);
  const rows = support?.list ? environments : listing.rows;
  if (!Array.isArray(rows)) return listStatusHtml(error);
  if (!support?.list) return legacyProjectsHtml(listing, device);
  return environments.map(environment => environmentHtml(environment, listing, device, error)).join("")
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
  const entries = [["support", environmentSupportAddress(device.id)], ["environments", environmentsAddress(device.id)], ["journalProjects", projectsAddress(device.id)]];
  const records = Object.fromEntries(entries.map(([kind, address]) => [kind, watchSettingsRecord(address, value => {
    const wasSupported = state.support?.list;
    state[kind] = value;
    paint();
    if (kind === "support" && value?.list && wasSupported === false) void refresh();
  })]));
  const reportError = error => { state.error = error.message; paint(); };
  records.projects = watchEnvironmentSettingsProjects({ deviceId: device.id, callRpc, active, onError: reportError,
    paint: value => { state.projects = value; paint(); } });
  const supported = () => active() && bridgeCapabilities(device.id).environments?.list === true;
  async function refreshEnvironments() {
    if (!supported()) return;
    const admission = await admitEnvironmentListWrite(device.id, supported);
    if (!admission || !supported()) return;
    const answer = await callRpc("environment.list");
    if (Array.isArray(answer?.environments)) await writeCachedEnvironments(device.id, answer.environments, admission, supported);
  }
  async function refresh() {
    await Promise.allSettled([
      records.projects.refresh().catch(reportError),
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
