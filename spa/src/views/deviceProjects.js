// The projects one machine holds: what is registered there, what each one
// pushes to, and adding another.
//
// A project is a fact of the daemon that registered it — its path, its base
// branch, the bare id that daemon minted — so every read and write here is that
// machine's own. The panel is handed one caller and the account's name for the
// machine behind it, and asks nothing else about devices.

import { esc } from "../core/text.js";
import { isolationLabel } from "../core/isolation.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { openSetRemote } from "../sheets/setRemote.js";
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
    <span class="premote">${project.remote ? "⇄ " + esc(project.remote) : '<span class="dim">no remote</span>'}</span>
    ${project.is_git === false ? "" : `<button class="btn mini setremote" data-id="${esc(project.project_id)}">Set remote…</button>`}</div>`;

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
    wireRemotes(projects);
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

  const wireRemotes = (projects) => {
    list.querySelectorAll(".setremote").forEach((button) => {
      button.onclick = () =>
        openSetRemote(
          projects.find((project) => project.project_id === button.dataset.id),
          refresh,
          { callRpc, deviceId },
        );
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
