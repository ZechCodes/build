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
 * Mounting is what reads, so a page that reconnects mounts again: a list read
 * over a connection that has gone is a list of what WAS there.
 */
export async function mountDeviceProjects(host, { callRpc, deviceId, deviceName, onProjectCreated }) {
  const panel = host.querySelector("[data-device-projects]");
  const list = panel.querySelector("#projlist");

  const refresh = async () => {
    try {
      const { projects } = await callRpc("project.list");
      list.innerHTML = projects.map(projectRowHtml).join("") || '<div class="dim" style="font-size:13px">No projects yet.</div>';
      wireRemotes(projects);
    } catch (error) {
      list.innerHTML = `<div class="adderr">${esc(error.message)}</div>`;
    }
  };

  const wireRemotes = (projects) => {
    list.querySelectorAll(".setremote").forEach((button) => {
      button.onclick = () =>
        openSetRemote(
          projects.find((project) => project.project_id === button.dataset.id),
          refresh,
          { callRpc },
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
