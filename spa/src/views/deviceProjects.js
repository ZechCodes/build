// The projects one machine holds: what is registered there, the folders each
// is cut from, and adding another. A project has no remote of its own; each of
// its sources has one, shown and edited in the project's settings (#228).
//
// A project is a fact of the daemon that registered it — its path, its base
// branch, the bare id that daemon minted — so every read and write here is that
// machine's own. The panel is handed one caller and the account's name for the
// machine behind it, and asks nothing else about devices.

import { esc } from "../core/text.js";
import { isolationLabel } from "../core/isolation.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { App } from "../app.js";
import { creationCall } from "../core/inboxDevices.js";
import { deviceProjectsAddress, projectSettingsAddress, watchSettingsRecord } from "../core/settingsRecords.js";
import { readCached, writeCached } from "../core/localCache.js";

export function deviceProjectsPanelHtml() {
  return `<div class="panel" data-device-projects>
      <h3>📁 Projects</h3>
      <div id="projlist"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="addproj">
        <button class="btn primary" id="newrepo">Add project…</button>
      </div>
    </div>`;
}

/** What a project does, as this machine will actually do it: a folder that is
 *  not a repository yet says so instead of naming a branch it has not got. */
const projectFactsHtml = (project) =>
  project.is_git === false
    ? "Folder · Git not initialized"
    : `${esc(project.base_branch)} · ${esc(isolationLabel(project.isolation_effective))}`;

const projectRowHtml = (project) => `
  <div class="projrow"><span class="pname">${esc(project.name)}</span>
    <span class="ppath">${esc(project.path)}</span>
    <span class="dim" style="font-size:11.5px">${projectFactsHtml(project)}</span>
    <span class="dim" style="font-size:11.5px">${sourceCountText(project)}</span>
    <button class="btn mini projsettings" data-id="${esc(project.project_id)}">Settings…</button></div>`;

const sourceCountText = (project) => {
  const count = (project.sources || []).length || 1;
  return count === 1 ? "1 source" : `${count} sources`;
};

/**
 * Mount the panel on one machine's caller, and read what that machine holds.
 *
 * The list paints from the cache and watches it, so every write to the
 * machine's projects repaints it; `refresh` is the pull that asks the machine
 * and writes its answer there. The panel stays mounted across a reconnect.
 */
export async function mountDeviceProjects(host, { callRpc, deviceId, deviceName, onProjectCreated }) {
  const panel = host.querySelector("[data-device-projects]");
  const list = panel.querySelector("#projlist");
  const record = watchSettingsRecord(deviceProjectsAddress(deviceId), (projects) => {
    if (!panel.isConnected || !Array.isArray(projects)) return;
    list.innerHTML = projects.map(projectRowHtml).join("") || '<div class="dim" style="font-size:13px">No projects yet.</div>';
    wireSettings();
  }, { owner: panel });

  const refresh = async () => {
    try {
      await record.pull(async () => {
        const { projects } = await callRpc("project.list");
        return projects;
      });
      const projects = (await readCached(deviceProjectsAddress(deviceId)))?.value || [];
      for (const project of projects) await writeCached(projectSettingsAddress(deviceId, project.project_id), project);
    } catch (error) {
      if (list.textContent.includes("loading…")) list.innerHTML = `<div class="adderr">${esc(error.message)}</div>`;
    }
  };

  const wireSettings = () => {
    list.querySelectorAll(".projsettings").forEach((button) => {
      button.onclick = () =>
        openProjectSettings(button.dataset.id, { callRpc, deviceId, onDeleted: refresh });
    });
  };

  panel.querySelector("#newrepo").onclick = () =>
    openNewRepo(
      async (project, target) => {
        await refresh();
        onProjectCreated?.(project, target);
      },
      {
        devices: App.devices,
        defaultDeviceId: deviceId,
        callRpcFor: (chosenId) => chosenId === deviceId ? callRpc : creationCall(chosenId),
      },
    );

  await refresh();
}
