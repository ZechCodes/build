import { $ } from "../dom.js";
import { notifyError } from "../core/notify.js";
import { confirmAction } from "../core/confirm.js";
import { esc } from "../core/text.js";
import { isolationFieldHtml, mountIsolation, projectIsolationTarget } from "../core/isolation.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { openBrowser } from "./browser.js";
import { deviceSettingsAddress, projectSettingsAddress, removeProjectSetting, watchSettingsRecord, writeProjectSetting } from "../core/settingsRecords.js";
import { deleteCached, readCached } from "../core/localCache.js";

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

/** Keep edits that have not been sent to the bridge when a project record
 * changes underneath this sheet. The add-source form is a second local draft. */
function focusedDraft(sheet) {
  const focused = sheet.ownerDocument.activeElement;
  return {
    focusId: focused && sheet.contains(focused) ? focused.id : "",
    selection: focused?.id === "psremote" ? [focused.selectionStart, focused.selectionEnd] : null,
  };
}

function sourceDraft(sheet) {
  const sourceUrl = sheet.querySelector("#psremoteurl");
  return sourceUrl ? { url: sourceUrl.value, name: sheet.querySelector("#pssourcename").value } : null;
}

function captureDraft(sheet, paintedRemote) {
  const remote = sheet.querySelector("#psremote");
  return {
    remote: remote && remote.value !== paintedRemote ? remote.value : null,
    ...focusedDraft(sheet),
    source: sourceDraft(sheet),
    remoteError: sheet.querySelector("#pserr")?.textContent || "",
    sourceError: sheet.querySelector("#pssrcerr")?.textContent || "",
  };
}

function restoreDraft(sheet, draft) {
  if (draft.remote !== null) sheet.querySelector("#psremote").value = draft.remote;
  if (draft.source) {
    sheet.querySelector("#psaddremote").click();
    sheet.querySelector("#psremoteurl").value = draft.source.url;
    sheet.querySelector("#pssourcename").value = draft.source.name;
  }
  sheet.querySelector("#pserr").textContent = draft.remoteError;
  sheet.querySelector("#pssrcerr").textContent = draft.sourceError;
  const focused = draft.focusId && sheet.querySelector(`#${draft.focusId}`);
  if (focused) {
    focused.focus();
    if (draft.selection) focused.setSelectionRange(...draft.selection);
  }
}

/** Opened with the caller of the machine this project is on: whoever opens the
 *  sheet has already resolved that, so nothing here asks which device it is. */
export function openProjectSettings(projectId, { callRpc, deviceId = "", onDeleted }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({ title: "Project settings", bodyHtml: '<div class="sub">Loading…</div>' });
  $("#scrim").classList.add("show");
  let frame = sheet.firstElementChild;
  let view = "settings";
  let paintedRemote = "";
  const current = () => sheet.isConnected && sheet.firstElementChild === frame && $("#scrim").classList.contains("show");
  const close = () => {
    record.dispose();
    $("#scrim").classList.remove("show");
  };

  const paintMissing = (message) => {
    if (!current() || view !== "settings") return;
    sheet.innerHTML = settingsSheetHtml({
      title: "Project settings",
      bodyHtml: `<div class="sub">${esc(message)}</div>
        <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button></div>`,
    });
    frame = sheet.firstElementChild;
    $("#pscancel").onclick = close;
  };

  const paint = (project) => {
    if (!current() || view !== "settings") return;
    const draft = captureDraft(sheet, paintedRemote);
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
    frame = sheet.firstElementChild;
    paintedRemote = project.remote || "";
    mountIsolation(sheet, { callRpc, target: projectIsolationTarget(project), deviceId, fromProjectRecord: true });
    mountSources(project, {
      callRpc, record, deviceId,
      onFrameChange: () => { frame = sheet.firstElementChild; view = "browser"; },
      onReturn: () => { view = "settings"; void record.read(); },
    });
    restoreDraft(sheet, draft);
    $("#pscancel").onclick = close;
    $("#psdelete").onclick = () => deleteProject(project, { callRpc, onDeleted, close, deviceId });
    $("#pssave").onclick = async () => {
      const save = $("#pssave");
      save.disabled = true;
      save.textContent = "saving…";
      $("#pserr").textContent = "";
      try {
        const changed = await callRpc("project.set_remote", { project_id: projectId, url: $("#psremote").value.trim() });
        if (changed?.project_id) await writeProjectSetting(deviceId, changed);
        close();
      } catch (e) {
        $("#pserr").textContent = e.message;
        save.disabled = false;
        save.textContent = "Save remote";
      }
    };
  };
  const record = watchSettingsRecord(projectSettingsAddress(deviceId, projectId), (project) => {
    if (project) paint(project);
  });
  void record.pull(async () => {
    const listed = await callRpc("project.list");
    const project = (listed.projects || []).find((candidate) => candidate.project_id === projectId);
    if (!project) throw new Error("This project is no longer registered on this device.");
    return project;
  }).catch((error) => {
    if (!$("#psname")) paintMissing(`Project settings are unavailable: ${error.message}`);
  });
}

/** The source controls: a Remove per folder, and the two ways one is added.
 *
 *  Every write answers the project row itself, so the sheet repaints from what
 *  the bridge said rather than from what it hoped. */
function mountSources(project, { callRpc, record, deviceId, onFrameChange, onReturn }) {
  const write = async (method, params, button) => {
    const error = $("#pssrcerr");
    error.textContent = "";
    button.disabled = true;
    try {
      const changed = await callRpc(method, params);
      await writeProjectSetting(deviceId, changed);
      await record.read();
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
  $("#psaddfolder").onclick = () => void browseForSource(project, { callRpc, deviceId, onFrameChange, onReturn });
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
async function browseForSource(project, { callRpc, deviceId, onFrameChange, onReturn }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({
    title: "Add folder",
    subtitleHtml: `Choose a folder to add to ${esc(project.name || "this project")}.`,
    bodyHtml: '<div id="psbrowser"></div><div class="row"><button class="btn" id="psbrowseback" type="button">Back</button></div><div class="adderr" id="psbrowseerr" role="alert"></div>',
  });
  onFrameChange();
  $("#psbrowseback").onclick = onReturn;
  const chosen = async (path) => {
    try {
      const changed = await callRpc("project.add_source", { project_id: project.project_id, path });
      await writeProjectSetting(deviceId, changed);
      onReturn();
    } catch (thrown) {
      const error = $("#psbrowseerr");
      if (error) error.textContent = thrown.message;
      else notifyError("The folder was not added", thrown.message);
    }
  };
  try {
    const address = deviceSettingsAddress(deviceId);
    let ready;
    const cached = new Promise((resolve) => { ready = resolve; });
    const settingsRecord = watchSettingsRecord(address, (settings) => {
      if (settings?.projects_dir) ready();
    });
    await settingsRecord.read();
    const pull = settingsRecord.pull(() => callRpc("settings.get"));
    const result = await Promise.race([
      cached.then(() => ({ ready: true })),
      pull.then(() => ({ ready: true }), (error) => ({ error })),
    ]);
    if (result.error) settingsRecord.dispose();
    else void pull.catch(() => {}).finally(settingsRecord.dispose);
    if (result.error) throw result.error;
    const startPath = (await readCached(address))?.value?.projects_dir;
    if (!$("#psbrowser")) return;
    await openBrowser({
      title: "Add folder",
      gitOnly: false,
      fallbackFromMissingStart: true,
      startPath,
      callRpc,
      deviceId,
      container: $("#psbrowser"),
      onCancel: onReturn,
      onChoose: (path) => void chosen(path),
    });
  } catch (thrown) {
    const error = $("#psbrowseerr");
    if (error) error.textContent = thrown.message;
  }
}

async function deleteProject(project, { callRpc, onDeleted, close, deviceId }) {
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
    await deleteCached([projectSettingsAddress(deviceId, project.project_id)]);
    await removeProjectSetting(deviceId, project.project_id);
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
