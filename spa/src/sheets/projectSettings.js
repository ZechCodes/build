import { $ } from "../dom.js";
import { notifyError } from "../core/notify.js";
import { confirmAction } from "../core/confirm.js";
import { esc } from "../core/text.js";
import { isolationFieldHtml, mountIsolation, projectIsolationTarget } from "../core/isolation.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { openBrowser } from "./browser.js";

const field = (label, id, value) =>
  `<div class="field"><label>${esc(label)}</label>
    <input id="${id}" style="width:100%" value="${esc(value || "")}" readonly /></div>`;

const sourceKind = (source) =>
  source.is_git === false ? "Folder" : `Git repository${source.base_branch ? ` · ${esc(source.base_branch)}` : ""}`;

const sourceHtml = (source, index) => `<div class="field"><label for="pssource-${index}">${esc(source.mount || source.name || `Folder ${index + 1}`)}</label>
    <input id="pssource-${index}" style="width:100%" value="${esc(source.path || source.remote || "")}" readonly>
    <div class="row"><div class="dim">${sourceKind(source)}</div>
      <button class="btn danger mini" type="button" style="margin-left:auto" data-remove-source="${esc(source.id)}">Remove</button></div></div>`;

/** The folders every NEW workspace is cut from. A workspace already standing
 *  keeps the directories it was cut with, which is why adding one here says so
 *  rather than pretending the change reaches back. */
const sourcesHtml = (project) => `<fieldset style="border:0;padding:0;margin:0"><legend>Workspace folders</legend>
  ${(project.sources || []).map(sourceHtml).join("")}
  <div class="dim">New workspaces are cut from these. A workspace that already exists keeps its own directories.</div>
  <div class="row"><button class="btn" id="psaddfolder" type="button">Add folder…</button>
    <button class="btn" id="psaddremote" type="button">Add Git remote…</button></div>
  <div id="psaddsource"></div>
  <div class="adderr" id="pssrcerr" role="alert"></div></fieldset>`;

/** Where to clone the remote from, and what to call the folder it lands in. A
 *  folder already on the device is picked in the browser instead. */
const addSourceHtml = () => `<div class="field">
    <label for="psremoteurl">Clone url</label>
    <input id="psremoteurl" placeholder="git@github.com:org/repo.git" style="width:100%" autocomplete="off">
    <label for="pssourcename">Name</label>
    <input id="pssourcename" style="width:100%" autocomplete="off" placeholder="What to call it">
    <div class="row"><button class="btn" id="pssourcecancel" type="button">Cancel</button>
      <button class="btn primary" id="pssourceadd" type="button" style="margin-left:auto">Add folder</button></div>
  </div>`;

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
    mountSources(project, { callRpc, paint });
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

/** The source controls: a Remove per folder, and the two ways one is added.
 *
 *  Every write answers the project row itself, so the sheet repaints from what
 *  the bridge said rather than from what it hoped. */
function mountSources(project, { callRpc, paint }) {
  const write = async (method, params, button) => {
    const error = $("#pssrcerr");
    error.textContent = "";
    button.disabled = true;
    try {
      paint(await callRpc(method, params));
    } catch (thrown) {
      button.disabled = false;
      if (error.isConnected) error.textContent = thrown.message;
      else notifyError("The project's folders were not changed", thrown.message);
    }
  };
  document.querySelectorAll("#sheet [data-remove-source]").forEach((button) => {
    button.onclick = () =>
      void write(
        "project.remove_source",
        { project_id: project.project_id, source_id: button.dataset.removeSource },
        button,
      );
  });
  $("#psaddremote").onclick = () => openAddRemote(project, write);
  $("#psaddfolder").onclick = () => void browseForSource(project, { callRpc, paint });
}

/** Say where to clone the remote from, and what to call it. */
function openAddRemote(project, write) {
  const host = $("#psaddsource");
  host.innerHTML = addSourceHtml();
  $("#pssourcecancel").onclick = () => { host.innerHTML = ""; $("#psaddremote").focus(); };
  $("#pssourceadd").onclick = () => {
    const name = $("#pssourcename").value.trim();
    const remote = $("#psremoteurl").value.trim();
    if (!remote) {
      $("#pssrcerr").textContent = "A Git remote needs a clone url.";
      return;
    }
    void write(
      "project.add_source",
      { project_id: project.project_id, remote, ...(name ? { name } : {}) },
      $("#pssourceadd"),
    );
  };
  $("#psremoteurl").focus();
}

/** Pick a folder on this device, in the browser the rest of the app uses. The
 *  sheet's body is handed over to it and comes back on Back or on a choice. */
async function browseForSource(project, { callRpc, paint }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({
    title: "Add folder",
    subtitleHtml: `Choose a folder to add to ${esc(project.name || "this project")}.`,
    bodyHtml: '<div id="psbrowser"></div><div class="row"><button class="btn" id="psbrowseback" type="button">Back</button></div><div class="adderr" id="psbrowseerr" role="alert"></div>',
  });
  $("#psbrowseback").onclick = () => paint(project);
  const chosen = async (path) => {
    try {
      paint(await callRpc("project.add_source", { project_id: project.project_id, path }));
    } catch (thrown) {
      const error = $("#psbrowseerr");
      if (error) error.textContent = thrown.message;
      else notifyError("The folder was not added", thrown.message);
    }
  };
  try {
    const startPath = (await callRpc("settings.get")).projects_dir;
    await openBrowser({
      title: "Add folder",
      gitOnly: false,
      fallbackFromMissingStart: true,
      startPath,
      callRpc,
      container: $("#psbrowser"),
      onCancel: () => paint(project),
      onChoose: (path) => void chosen(path),
    });
  } catch (thrown) {
    const error = $("#psbrowseerr");
    if (error) error.textContent = thrown.message;
  }
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
