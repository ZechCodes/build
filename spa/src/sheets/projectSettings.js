import { $ } from "../dom.js";
import { notifyError } from "../core/notify.js";
import { confirmAction } from "../core/confirm.js";
import { esc } from "../core/text.js";
import { isolationFieldHtml, mountIsolation, projectIsolationTarget } from "../core/isolation.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { openBrowser } from "./browser.js";
import { deviceSettingsAddress, projectSettingsAddress, removeProjectSetting, watchSettingsRecord, writeProjectSetting } from "../core/settingsRecords.js";
import { deleteCached, readCached, subscribeCache } from "../core/localCache.js";
import { uiAddress, watchUiState } from "../core/localUiState.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { refreshGithubRepos } from "../core/githubRepos.js";
import { attachRepoPicker, disposeRepoPickers } from "./repoPicker.js";
import { readSourceEditSupport, SOURCE_EDIT_SUPPORT_KIND } from "../core/sourceEditSupport.js";
import { mountSourceCards, readSourceEdits, restoreSourceEdits, sourceEditing, sourcesSectionHtml } from "./projectSources.js";

/** The project's own name. It is the first source's folder's, and is not a
 *  source's label: a source is renamed on its card. */
const generalHtml = (project) => `<section class="ps-section" aria-labelledby="ps-general-h"><h4 id="ps-general-h">General</h4>
    <div class="field"><label for="psproject">Project name</label>
      <input id="psproject" style="width:100%" ${fieldTraits("identifier")} value="${esc(project.name || "")}" readonly>
      <div class="dim ps-hint">Named after the project's first folder.</div></div></section>`;

const isolationHtml = () => `<section class="ps-section" aria-labelledby="ps-isolation-h"><h4 id="ps-isolation-h">Isolation</h4>
    ${isolationFieldHtml()}</section>`;

const dangerHtml = () => `<section class="ps-section ps-danger" aria-labelledby="ps-danger-h"><h4 id="ps-danger-h">Danger zone</h4>
    <p class="sub">Delete this project and all of its workspaces from Build. Files and unsaved changes in Build-managed workspaces will be permanently removed. Original project folders and external checkouts are kept.</p>
    <button class="btn danger" id="psdelete">Delete project…</button>
    <div class="adderr" id="pserr" role="alert"></div></section>`;

/** Where to clone the remote from, and what to call the folder it lands in. A
 *  folder already on the device is picked in the browser instead. */
const addSourceHtml = () => `<div class="field">
    <label for="psremoteurl">Clone url</label>
    <input id="psremoteurl" placeholder="git@github.com:org/repo.git" style="width:100%" ${fieldTraits("identifier")}>
    <label for="pssourcelabel">Folder label</label>
    <input id="pssourcelabel" style="width:100%" ${fieldTraits("identifier")} placeholder="What to call it">
    <div class="row"><button class="btn" id="pssourcecancel" type="button">Cancel</button>
      <button class="btn primary" id="pssourceadd" type="button" style="margin-left:auto">Add folder</button></div>
  </div>`;

/** Keep edits that have not been sent to the bridge when a project record
 * changes underneath this sheet: every card's, and the add-source form's. */
function focusedDraft(sheet) {
  const focused = sheet.ownerDocument.activeElement;
  const typed = focused && sheet.contains(focused) && typeof focused.selectionStart === "number";
  return {
    focusId: focused && sheet.contains(focused) ? focused.id : "",
    selection: typed ? [focused.selectionStart, focused.selectionEnd] : null,
  };
}

function sourceDraft(sheet) {
  const sourceUrl = sheet.querySelector("#psremoteurl");
  return sourceUrl ? { url: sourceUrl.value, name: sheet.querySelector("#pssourcelabel").value } : null;
}

function captureDraft(sheet, project) {
  return {
    edits: project ? readSourceEdits(sheet, project) : {},
    ...focusedDraft(sheet),
    source: sourceDraft(sheet),
    sourceError: sheet.querySelector("#pssrcerr")?.textContent || "",
  };
}

const draftPart = (live, saved, displayed) =>
  live !== null && JSON.stringify(live) !== JSON.stringify(displayed) ? live : saved ?? null;

const combineDraft = (live, saved, displayed = {}) => ({
  ...saved,
  ...live,
  edits: draftPart(live.edits, saved?.edits, displayed.edits ?? {}) || {},
  source: draftPart(live.source, saved?.source, displayed.source),
  focusId: live.focusId || saved?.focusId || "",
});

function restoreAddSource(sheet, source) {
  if (source) {
    if (!sheet.querySelector("#psremoteurl")) sheet.querySelector("#psaddremote").click();
    sheet.querySelector("#psremoteurl").value = source.url;
    sheet.querySelector("#pssourcelabel").value = source.name;
  } else if (sheet.querySelector("#psremoteurl")) {
    disposeRepoPickers(sheet.querySelector("#psaddsource"));
    sheet.querySelector("#psaddsource").innerHTML = "";
  }
}

function restoreFocus(sheet, draft) {
  const found = draft.focusId && sheet.ownerDocument.getElementById(draft.focusId);
  const focused = found && sheet.contains(found) ? found : null;
  const free = sheet.ownerDocument.activeElement === sheet.ownerDocument.body || sheet.contains(sheet.ownerDocument.activeElement);
  if (!focused || !free) return;
  focused.focus();
  if (draft.selection && typeof focused.setSelectionRange === "function") focused.setSelectionRange(...draft.selection);
}

function restoreDraft(sheet, project, draft) {
  restoreSourceEdits(sheet, project, draft.edits);
  restoreAddSource(sheet, draft.source);
  sheet.querySelector("#pssrcerr").textContent = draft.sourceError || "";
  restoreFocus(sheet, draft);
}

/** Opened with the caller of the machine this project is on: whoever opens the
 *  sheet has already resolved that, so nothing here asks which device it is. */
export function openProjectSettings(projectId, { callRpc, deviceId = "", onDeleted }) {
  const sheet = $("#sheet");
  disposeRepoPickers(sheet);
  sheet.innerHTML = settingsSheetHtml({ title: "Project settings", bodyHtml: '<div class="sub">Loading…</div>' });
  $("#scrim").classList.add("show");
  let frame = sheet.firstElementChild;
  let view = "settings";
  let painted = null;
  let editsSources = false;
  const notices = {};
  let cachedDraft = null;
  let displayedDraft = {};
  let draftRecord;
  const saveDraft = (debounced = false) => {
    const snapshot = captureDraft(sheet, painted);
    if (debounced) draftRecord?.schedule(snapshot);
    else void draftRecord?.write(snapshot);
  };
  const current = () => sheet.isConnected && sheet.firstElementChild === frame && $("#scrim").classList.contains("show");
  // Asked again when the machine greets while THIS opening is on screen.
  const repoAsk = refreshGithubRepos(deviceId, callRpc, { wanted: current });
  // Whether this machine edits sources in place is a fact a greeting writes
  // to the cache; the cards repaint when it lands.
  const readSupport = async () => {
    editsSources = await readSourceEditSupport(deviceId);
    if (painted) paint(painted);
  };
  const unsubscribeSupport = subscribeCache({ deviceId }, (address) => {
    if (address?.kind === SOURCE_EDIT_SUPPORT_KIND) void readSupport();
  });
  const close = () => {
    repoAsk.stop();
    unsubscribeSupport();
    disposeRepoPickers(sheet);
    record.dispose();
    draftRecord?.dispose();
    $("#scrim").classList.remove("show");
  };

  const paintMissing = (message) => {
    if (!current() || view !== "settings") return;
    disposeRepoPickers(sheet);
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
    const live = captureDraft(sheet, painted);
    const draft = combineDraft(live, cachedDraft, displayedDraft);
    const editing = sourceEditing(editsSources);
    disposeRepoPickers(sheet);
    sheet.innerHTML = settingsSheetHtml({
      title: "Project settings",
      bodyHtml: `${generalHtml(project)}
      ${sourcesSectionHtml(project, { editing, editsSources, notices })}
      ${isolationHtml()}
      ${dangerHtml()}
      <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button></div>`,
    });
    frame = sheet.firstElementChild;
    painted = project;
    mountIsolation(sheet, { callRpc, target: projectIsolationTarget(project), deviceId, fromProjectRecord: true });
    mountSources(project, {
      callRpc, record, deviceId, editing,
      saveDraft,
      onSaved: (sourceId, notice) => { notices[sourceId] = notice; },
      onFrameChange: () => { frame = sheet.firstElementChild; view = "browser"; },
      onReturn: () => { view = "settings"; void record.read(); },
    });
    restoreDraft(sheet, project, draft);
    if (draft.edits === cachedDraft?.edits) displayedDraft.edits = draft.edits;
    if (draft.source === cachedDraft?.source) displayedDraft.source = draft.source;
    $("#pscancel").onclick = close;
    $("#psdelete").onclick = () => deleteProject(project, { callRpc, onDeleted, close, deviceId });
  };
  const record = watchSettingsRecord(projectSettingsAddress(deviceId, projectId), (project) => {
    if (project) paint(project);
  });
  draftRecord = watchUiState(uiAddress({ deviceId, entityId: projectId, view: "project-settings", kind: "draft" }), (saved) => {
    if (!saved || !current()) return;
    cachedDraft = saved;
    if (view !== "settings" || !painted || !sheet.querySelector("#psproject")) return;
    const live = captureDraft(sheet, painted);
    restoreDraft(sheet, painted, combineDraft(live, saved, displayedDraft));
    displayedDraft = { edits: saved.edits ?? {}, source: saved.source };
  }, { debounceMs: 180 });
  void readSupport();
  void record.pull(async () => {
    const listed = await callRpc("project.list");
    const project = (listed.projects || []).find((candidate) => candidate.project_id === projectId);
    if (!project) throw new Error("This project is no longer registered on this device.");
    return project;
  }).catch((error) => {
    // The refusal is the sentence: the sheet's title already says what it is.
    if (!$("#psproject")) paintMissing(error.message);
  });
  return { whenCachePainted: record.whenPainted };
}

/** The source controls: the cards, and the two ways a source is added.
 *
 *  Every write answers the project row itself, so the sheet repaints from what
 *  the bridge said rather than from what it hoped. */
function mountSources(project, { callRpc, record, deviceId, editing, saveDraft, onSaved, onFrameChange, onReturn }) {
  const write = async (send, button, error, onDone) => {
    if (error) error.textContent = "";
    button.disabled = true;
    try {
      const changed = await send();
      if (changed?.project_id) await writeProjectSetting(deviceId, changed);
      await onDone?.(changed);
      await record.read();
      saveDraft();
      return changed;
    } catch (thrown) {
      button.disabled = false;
      if (error?.isConnected) error.textContent = thrown.message;
      else notifyError("The project's folders were not changed", thrown.message);
      return null;
    }
  };
  mountSourceCards($("#sheet"), project, { editing, callRpc, deviceId, write, saveDraft, onSaved });
  $("#psaddremote").onclick = () => openAddRemote(project, { write, callRpc, saveDraft, deviceId });
  $("#psaddfolder").onclick = () => void browseForSource(project, { callRpc, deviceId, onFrameChange, onReturn });
}

/** Say where to clone the remote from, and what to call it. */
function openAddRemote(project, { write, callRpc, saveDraft, deviceId }) {
  const host = $("#psaddsource");
  disposeRepoPickers(host);
  host.innerHTML = addSourceHtml();
  attachRepoPicker($("#psremoteurl"), deviceId);
  $("#pssourcecancel").onclick = () => { disposeRepoPickers(host); host.innerHTML = ""; saveDraft(); $("#psaddremote").focus(); };
  $("#psremoteurl").oninput = () => saveDraft(true);
  $("#pssourcelabel").oninput = () => saveDraft(true);
  $("#pssourceadd").onclick = () => {
    const name = $("#pssourcelabel").value.trim();
    const remote = $("#psremoteurl").value.trim();
    if (!remote) {
      $("#pssrcerr").textContent = "A Git remote needs a clone url.";
      return;
    }
    void write(
      () => callRpc("project.add_source", { project_id: project.project_id, remote, ...(name ? { name } : {}) }),
      $("#pssourceadd"),
      $("#pssrcerr"),
    );
  };
  $("#psremoteurl").focus();
}

/** Pick a folder on this device, in the browser the rest of the app uses. The
 *  sheet's body is handed over to it and comes back on Back or on a choice. */
async function browseForSource(project, { callRpc, deviceId, onFrameChange, onReturn }) {
  const sheet = $("#sheet");
  disposeRepoPickers(sheet);
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
