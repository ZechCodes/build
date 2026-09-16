import { $ } from "../dom.js";
import { notifyError } from "../core/notify.js";
import { confirmAction } from "../core/confirm.js";
import { esc } from "../core/text.js";
import { isolationFieldHtml, mountIsolation, projectIsolationTarget } from "../core/isolation.js";
import { settingsSheetHtml } from "./settingsSheet.js";

const field = (label, id, value) =>
  `<div class="field"><label>${esc(label)}</label>
    <input id="${id}" style="width:100%" value="${esc(value || "")}" readonly /></div>`;

const sourcesHtml = (project) => !project.sources?.length ? "" : `<fieldset style="border:0;padding:0;margin:0"><legend>Workspace folders</legend>
  ${project.sources.map((source, index) => `<div class="field"><label for="pssource-${index}">${esc(source.mount || source.name || `Folder ${index + 1}`)}</label>
    <input id="pssource-${index}" style="width:100%" value="${esc(source.path || source.remote || "")}" readonly>
    <div class="dim">${source.is_git === false ? "Folder" : `Git repository${source.base_branch ? ` · ${esc(source.base_branch)}` : ""}`}</div></div>`).join("")}</fieldset>`;

/** Opened with the caller of the machine this project is on: whoever opens the
 *  sheet has already resolved that, so nothing here asks which device it is. */
export function openProjectSettings(projectId, { callRpc, onDeleted }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({ title: "Project settings", bodyHtml: '<div class="sub">Loading…</div>' });
  $("#scrim").classList.add("show");

  const close = () => $("#scrim").classList.remove("show");

  const paintMissing = (message) => {
    sheet.innerHTML = settingsSheetHtml({
      title: "Project settings",
      bodyHtml: `<div class="sub">${esc(message)}</div>
        <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button></div>`,
    });
    $("#pscancel").onclick = close;
  };

  const paint = (project) => {
    sheet.innerHTML = settingsSheetHtml({
      title: "Project settings",
      subtitleHtml: "Name, location and base branch come from the repository Build was pointed at.",
      bodyHtml: `
      ${field("Name", "psname", project.name)}
      ${field("Repository path", "pspath", project.path)}
      ${field("Base branch", "psbranch", project.base_branch)}
      ${sourcesHtml(project)}
      ${isolationFieldHtml()}
      <div class="field"><label>Origin remote</label>
        <input id="psremote" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(project.remote || "")}" /></div>
      <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button>
        <button class="btn primary" id="pssave">Save remote</button></div>
      <div class="adderr" id="pserr"></div>
      <section class="field" style="margin-top:24px;border-top:1px solid var(--line);padding-top:16px">
        <h4>Delete project</h4>
        <p class="sub">Delete this project and all of its workspaces from Build. Files and unsaved changes in Build-managed workspaces will be permanently removed. Original project folders and external checkouts are kept.</p>
        <button class="btn danger" id="psdelete">Delete project…</button>
      </section>`,
    });
    mountIsolation(sheet, { callRpc, target: projectIsolationTarget(project), settings: project });
    $("#pscancel").onclick = close;
    $("#psdelete").onclick = () => deleteProject(project, { callRpc, onDeleted, close });
    $("#pssave").onclick = async () => {
      const save = $("#pssave");
      save.disabled = true;
      save.textContent = "saving…";
      $("#pserr").textContent = "";
      try {
        await callRpc("project.set_remote", { project_id: projectId, url: $("#psremote").value.trim() });
        close();
      } catch (e) {
        $("#pserr").textContent = e.message;
        save.disabled = false;
        save.textContent = "Save remote";
      }
    };
  };

  callRpc("project.list")
    .then((listed) => {
      const project = (listed.projects || []).find((candidate) => candidate.project_id === projectId);
      if (project) paint(project);
      else paintMissing("This project is no longer registered on this device.");
    })
    .catch((e) => paintMissing(`Project settings are unavailable: ${e.message}`));
}

async function deleteProject(project, { callRpc, onDeleted, close }) {
  const button = $("#psdelete");
  const sheet = button.closest("#sheet");
  const errorMessage = sheet.querySelector("#pserr");
  errorMessage.textContent = "";
  button.disabled = true;
  const confirmed = await confirmAction({
    title: `Delete ${project.name || "project"}?`,
    warnings: ["This cannot be undone. Files and unsaved changes in Build-managed workspaces will be permanently removed."],
    actions: ["Delete all workspaces belonging to this project.", "Remove the project from Build. Original project folders and external checkouts are kept."],
    confirmLabel: "Delete project and workspaces",
    danger: true,
  });
  if (!confirmed) {
    button.disabled = false;
    button.focus();
    return;
  }
  button.textContent = "Deleting…";
  const controls = [...sheet.querySelectorAll("button, input, select")];
  const disabled = controls.map((control) => control.disabled);
  controls.forEach((control) => { control.disabled = true; });
  try {
    await callRpc("project.delete", { project_id: project.project_id, confirm: true });
    if (sheet.contains(button)) close();
  } catch (error) {
    errorMessage.textContent = error.message;
    controls.forEach((control, index) => { control.disabled = disabled[index]; });
    button.disabled = false;
    button.textContent = "Delete project…";
    if (!sheet.contains(button) || !$("#scrim").classList.contains("show")) {
      notifyError("Project deletion failed", error.message);
    }
    return;
  }
  try {
    await onDeleted?.(project);
  } catch (error) {
    notifyError("Project deleted, but refreshing the project list failed", error.message);
  }
}
