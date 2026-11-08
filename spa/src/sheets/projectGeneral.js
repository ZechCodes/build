import { esc } from "../core/text.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { requireEnvironmentAction, writeEnvironmentProject, projectIsArchived } from "../core/environmentWrites.js";

export function projectGeneralHtml(project, { rename, environmentName, deviceId }) {
  const editable = rename && !projectIsArchived(project);
  const environment = project.environment_id ? `<p>In environment ${rename ? `<button class="btn mini" data-project-environment>${esc(environmentName)}</button>` : esc(environmentName)}</p>` : "";
  return `<section class="ps-section" aria-labelledby="ps-general-h"><h4 id="ps-general-h">General</h4>
    <div class="field"><label for="psproject">Project name</label>
    <input id="psproject" style="width:100%" ${fieldTraits("identifier")} value="${esc(project.name || "")}"${editable ? "" : " readonly"}>
    ${editable ? '<button class="btn" id="psrename">Save name</button>' : '<div class="dim ps-hint">Named after the project\'s first folder.</div>'}</div>
    ${environment}${deviceId ? `<p class="dim">Device: ${esc(deviceId)}</p>` : ""}<div class="adderr" id="psrenameerr" role="alert"></div></section>`;
}

export function projectArchiveHtml(project, support, refusal) {
  if (!support.archive) return "";
  const archived = projectIsArchived(project);
  return `<section class="ps-section"><h4>${archived ? "Archived" : "Archive"}</h4>
    <p class="sub">${archived ? "Restore this project to make changes." : "Keep this project's history and hide it from active work."}</p>
    <button class="btn" id="psarchive"${refusal ? " disabled" : ""}>${archived ? "Restore" : "Archive project"}</button>
    <div class="adderr" id="psarchiveerr" role="alert">${esc(refusal)}</div></section>`;
}

export function mountProjectGeneral(sheet, project, { deviceId, callRpc, onArchiveRefused, openEnvironment }) {
  const environment = sheet.querySelector("[data-project-environment]");
  if (environment) environment.onclick = openEnvironment;
  const edit = sheet.querySelector("[data-edit-environment]");
  if (edit) edit.onclick = openEnvironment;
  const rename = sheet.querySelector("#psrename");
  if (rename) rename.onclick = () => void write(rename, sheet.querySelector("#psrenameerr"), async () => {
    requireEnvironmentAction(deviceId, "rename");
    return callRpc("project.rename", { project_id: project.project_id, name: sheet.querySelector("#psproject").value.trim() });
  });
  const archive = sheet.querySelector("#psarchive");
  if (archive) archive.onclick = () => void write(archive, sheet.querySelector("#psarchiveerr"), async () => {
    requireEnvironmentAction(deviceId, "archive");
    return projectIsArchived(project) ? callRpc("project.restore", { project_id: project.project_id }) : callRpc("project.archive", { project_id: project.project_id });
  }, onArchiveRefused);
  async function write(button, error, send, refused) {
    button.disabled = true; error.textContent = "";
    try { await writeEnvironmentProject(deviceId, await send()); }
    catch (failure) {
      error.textContent = failure.message;
      if (refused) refused(failure.message);
      else button.disabled = false;
    }
  }
}
