import { archiveBlockersObservable } from "./archiveBlockerChanges.js";
import { App } from "../app.js";
import { esc } from "../core/text.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { environmentAction, writeEnvironmentProject, projectIsArchived } from "../core/environmentWrites.js";

export function projectGeneralHtml(project, { projectRename, environmentRename, environmentName, deviceId }) {
  const editable = projectRename && !projectIsArchived(project);
  const environment = projectEnvironmentHtml(project, environmentName, environmentRename);
  return `<section class="ps-section" aria-labelledby="ps-general-h"><h4 id="ps-general-h">General</h4>
    <div class="field"><label for="psproject">Project name</label>
    <input id="psproject" style="width:100%" ${fieldTraits("identifier")} value="${esc(project.name || "")}"${editable ? "" : " readonly"}>
    ${editable ? '<button class="btn" id="psrename">Save name</button>' : '<div class="dim ps-hint">Named after the project\'s first folder.</div>'}</div>
    ${environment}${deviceId ? `<p class="dim">Device: ${esc(deviceLabelOf(deviceId))}</p>` : ""}<div class="adderr" id="psrenameerr" role="alert"></div></section>`;
}

export function projectArchiveHtml(project, support, refusal) {
  if (!support.archive) return "";
  const archived = projectIsArchived(project);
  return `<section class="ps-section"><h4>${archived ? "Archived" : "Archive"}</h4>
    <p class="sub">${archived ? "Restore this project to make changes." : "Keep this project's history and hide it from active work."}</p>
    <button class="btn" id="psarchive"${refusal && !archived ? " disabled" : ""}>${archived ? "Restore" : "Archive project"}</button>
    <div class="adderr" id="psarchiveerr" role="alert">${esc(refusal?.message || "")}</div></section>`;
}

export function mountProjectGeneral(sheet, project, { deviceId, callRpc, onArchiveRefused, beforeArchive, openEnvironment }) {
  const environment = sheet.querySelector("[data-project-environment]");
  if (environment) environment.onclick = openEnvironment;
  const edit = sheet.querySelector("[data-edit-environment]");
  if (edit) edit.onclick = openEnvironment;
  const rename = sheet.querySelector("#psrename");
  if (rename) rename.onclick = () => void write(rename, sheet.querySelector("#psrenameerr"), async () => {
    const name = sheet.querySelector("#psproject").value.trim();
    return environmentAction(deviceId, "projectRename", () => callRpc("project.rename", { project_id: project.project_id, name }));
  });
  const archive = sheet.querySelector("#psarchive");
  let baseline;
  if (archive) archive.onclick = () => void write(archive, sheet.querySelector("#psarchiveerr"), async () => {
    if (!projectIsArchived(project)) baseline = await beforeArchive();
    return environmentAction(deviceId, "archive", () => projectIsArchived(project) ? callRpc("project.restore", { project_id: project.project_id }) : callRpc("project.archive", { project_id: project.project_id }));
  }, failure => {
    if (projectIsArchived(project) || !archiveBlockersObservable(failure.details?.blockers)) return false;
    return onArchiveRefused({ message: failure.message, blockers: failure.details.blockers }, baseline);
  });
  async function write(button, error, send, refused) {
    button.disabled = true; error.textContent = "";
    try { await writeEnvironmentProject(deviceId, await send()); }
    catch (failure) {
      error.textContent = failure.message;
      button.disabled = await refused?.(failure) === true;
    }
  }
}

const deviceLabelOf = id => App.devices?.find(device => device.id === id)?.name || id;
function projectEnvironmentHtml(project, name, editable) {
  if (!project.environment_id) return "";
  const label = name || project.environment_name || project.environment_id;
  return `<p>In environment ${editable ? `<button class="btn mini" data-project-environment>${esc(label)}</button>` : esc(label)}</p>`;
}
