// The cog at the right of the view-area toolbar: what this workspace is
// called, what its agents start on, and the one way to take it away.
//
// Three things, in the order a person reaches for them. The NAME is the label
// and nothing else — the folder on disk and the branches inside it are what
// every terminal, checkout and running agent already holds, so Build renames
// the record and leaves the tree exactly where it is. The DEFAULTS are the
// account's own panel (core/harnessDefaults.js) pointed at this workspace's
// slot, so the two layer rather than compete (core/workspaceDefaults.js). The
// DELETE is the opposite end of New workspace: agents and terminals stop, the
// checkouts are handed back to the repositories they were cut from, and the
// root goes with them.
//
// Opened with the caller of the machine this workspace is on — whoever opens
// the sheet has already resolved that, exactly as the project settings sheet
// (sheets/projectSettings.js) is opened, so nothing here asks which device.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError } from "../core/notify.js";
import { harnessDefaultsPanelHtml, mountHarnessDefaults } from "../core/harnessDefaults.js";
import { forgetWorkspaceDefaults, workspaceDefaultsStorage } from "../core/workspaceDefaults.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { deviceModelsAddress, projectSettingsAddress, watchSettingsRecord, workspaceSettingsAddress } from "../core/settingsRecords.js";
import { deleteCached } from "../core/localCache.js";
import { uiAddress, watchUiState } from "../core/localUiState.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { refreshGithubRepos } from "../core/githubRepos.js";
import { attachRepoPicker } from "./repoPicker.js";

/** The defaults panel's own element ids. Distinct from the account page's
 *  `def`, because both panels can be in one document. */
const PREFIX = "wsdef";

const DEFAULTS_BLURB =
  "What a new agent in THIS workspace starts on. Anything left blank falls through to your account-wide defaults in Settings.";

const nameFieldHtml = (name) => `
    <div class="field">
      <label for="wslabel">Workspace label</label>
      <input id="wslabel" style="width:100%" value="${esc(name || "")}" ${fieldTraits("identifier")} />
      <div class="dim">What this workspace is called in Build. The folder on disk and the branches inside it keep the names they were cut with.</div>
    </div>
    <div class="row">
      <button class="btn" id="wscancel">Close</button>
      <button class="btn primary" id="wssave" style="margin-left:auto" disabled>Save name</button>
    </div>
    <div class="adderr" id="wserr" role="alert"></div>`;

/** The workspace's own folders, filled in once the machine answers. What it was
 *  cut with plus whatever was added since — the project's sources are the
 *  offer, not the contents. */
const directoriesHtml = () => `
    <section class="field" id="wsdirs" style="margin-top:16px">
      <h4>Directories</h4>
      <div class="sub">Loading…</div>
    </section>`;

const directoryRowHtml = (directory) => `<div class="row" style="align-items:baseline">
      <div><div>${esc(directory.name || directory.source_id || "directory")}</div>
        <div class="dim">${esc(directory.path || "")}${directory.branch ? ` · ${esc(directory.branch)}` : ""}${directory.status && directory.status !== "ready" ? ` · ${esc(directory.status)}` : ""}</div></div>
      <button class="btn danger mini" type="button" style="margin-left:auto" data-remove-directory="${esc(directory.id)}">Remove</button>
    </div>`;

/** The offer: every project source this workspace was not cut with, and the two
 *  ways to name a folder that is nobody's source. */
const addDirectoryHtml = (offered) => `<div class="field">
      <label for="wsdiradd">Add a directory</label>
      <select id="wsdiradd">
        <option value="">Choose what to add…</option>
        ${offered.map((source) => `<option value="${esc(source.id)}">${esc(source.name || source.id)} (project folder)</option>`).join("")}
        <option value="path">A folder on this device…</option>
        <option value="remote">A Git remote…</option>
      </select>
      <div id="wsdirfields"></div>
      <div class="row"><button class="btn primary" id="wsdiraddgo" type="button" style="margin-left:auto" disabled>Add directory</button></div>
    </div>`;

const directoryLabelHtml = `<label for="wsdirlabel">Folder label</label><input id="wsdirlabel" style="width:100%" ${fieldTraits("identifier")}>`;

const directoryFieldsHtml = (kind) => {
  if (kind === "path") {
    return `<label for="wsdirpath">Folder</label><input id="wsdirpath" style="width:100%" placeholder="/home/you/code/docs" ${fieldTraits("identifier", "next")}>${directoryLabelHtml}`;
  }
  if (kind === "remote") {
    return `<label for="wsdirremote">Clone url</label><input id="wsdirremote" style="width:100%" placeholder="git@github.com:org/repo.git" ${fieldTraits("identifier", "next")}>${directoryLabelHtml}`;
  }
  return "";
};

const directoriesBodyHtml = (detail, offered) => {
  const directories = detail.directories || [];
  return `<h4>Directories</h4>
      <p class="sub">The folders this workspace holds. Removing one hands its checkout back to the repository it was cut from and takes the folder; anything in it that is nowhere else goes with it.</p>
      ${directories.length ? directories.map(directoryRowHtml).join("") : '<div class="sub">This workspace holds no directories.</div>'}
      ${addDirectoryHtml(offered)}
      <div class="adderr" id="wsdirerr" role="alert"></div>`;
};

const dangerZoneHtml = () => `
    <section class="field" style="margin-top:24px;border-top:1px solid var(--line);padding-top:16px">
      <h4>Delete workspace</h4>
      <p class="sub">Stop this workspace's agents and terminals, remove its checkouts from disk, and drop it from Build. The project's own repositories are left exactly as they are.</p>
      <button class="btn danger" id="wsdelete">Delete workspace…</button>
    </section>`;

const emptyDirectoryDraft = () => ({ kind: "", path: "", remote: "", name: "" });
/** Each directory draft field and the input that holds it. */
const DIRECTORY_INPUTS = { path: "#wsdirpath", remote: "#wsdirremote", name: "#wsdirlabel" };
const directoryDraftOf = (host) => ({
  kind: host.querySelector("#wsdiradd")?.value || "",
  ...Object.fromEntries(Object.entries(DIRECTORY_INPUTS).map(([field, selector]) => [field, host.querySelector(selector)?.value || ""])),
});

function restoreDirectoryDraft(host, draft, deviceId) {
  const choice = host.querySelector("#wsdiradd");
  if (!choice || !draft) return;
  choice.value = draft.kind || "";
  host.querySelector("#wsdirfields").innerHTML = directoryFieldsHtml(choice.value);
  host.querySelector("#wsdiraddgo").disabled = !choice.value;
  for (const [field, selector] of Object.entries(DIRECTORY_INPUTS)) {
    const control = host.querySelector(selector);
    if (control) control.value = draft[field] || "";
  }
  attachRepoPicker(host.querySelector(DIRECTORY_INPUTS.remote), deviceId);
}

/**
 * Open the workspace settings sheet.
 *
 * `workspace` is one row as the feed names it: `{ id, name, workspaceKey }` —
 * the bare id for the wire, and the account-wide key the browser-local
 * defaults are stored under. `catalog` may be a promise; the defaults panel
 * fills in when that machine answers, and a sheet closed first takes its
 * question with it.
 */
export function openWorkspaceSettings(workspace, { callRpc, catalog, deviceId = workspace.workspaceKey?.split("/")[0] || "", storage = localStorage, onRenamed, onDeleted }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({
    title: "Workspace settings",
    bodyHtml: `${nameFieldHtml(workspace.name)}
      ${directoriesHtml()}
      ${harnessDefaultsPanelHtml({ prefix: PREFIX, title: "🤖 Agent defaults here", blurb: DEFAULTS_BLURB })}
      ${dangerZoneHtml()}`,
  });
  $("#scrim").classList.add("show");
  let disposeDirectories = () => {};
  let disposeCatalog = () => {};
  const draft = { name: null, directory: emptyDirectoryDraft() };
  let draftRecord;
  const saveDraft = (debounced = false) => {
    const snapshot = { ...draft, directory: { ...draft.directory } };
    if (debounced) { draftRecord.schedule(snapshot); return Promise.resolve(); }
    return draftRecord.write(snapshot);
  };
  let repoAsk = null;
  const close = () => {
    repoAsk?.stop();
    disposeDirectories();
    disposeCatalog();
    draftRecord.dispose();
    $("#scrim").classList.remove("show");
  };
  const opened = sheet.firstElementChild;
  /** Still the sheet this call opened: an answer that lands after the reader
   *  moved on must not write into whatever is on screen now. */
  const current = () => sheet.isConnected && sheet.firstElementChild === opened;
  // Asked again when the machine greets while THIS opening is on screen.
  repoAsk = refreshGithubRepos(deviceId, callRpc, { wanted: () => current() && $("#scrim").classList.contains("show") });

  draftRecord = watchUiState(uiAddress({ deviceId, entityId: workspace.id, view: "workspace-settings", kind: "draft" }), (saved) => {
    if (!current() || !saved) return;
    if (JSON.stringify(saved) === JSON.stringify(draft)) return;
    draft.name = typeof saved.name === "string" ? saved.name : null;
    draft.directory = saved.directory || emptyDirectoryDraft();
    if (draft.name !== null) $("#wslabel").value = draft.name;
    nameChanged();
    restoreDirectoryDraft(sheet, draft.directory, deviceId);
  }, { debounceMs: 180 });
  const clearDraft = async () => draftRecord.write({ name: null, directory: emptyDirectoryDraft() });

  $("#wscancel").onclick = close;
  const nameChanged = wireName(workspace, { callRpc, close, onRenamed, draft, saveDraft, clearDraft });
  $("#wsdelete").onclick = () => void deleteWorkspace(workspace, { callRpc, close, onDeleted, storage, deviceId });
  disposeDirectories = mountDirectories(workspace, { callRpc, current, deviceId, draft, saveDraft });
  const defaultsStorage = workspaceDefaultsStorage(workspace.workspaceKey, storage);
  let cacheCatalogSeen = false;
  const catalogRecord = watchSettingsRecord(deviceModelsAddress(deviceId), (offered) => {
    if (!current() || !offered) return;
    cacheCatalogSeen = true;
    const focused = sheet.ownerDocument.activeElement;
    const defaultsPanel = sheet.querySelector(`[data-harness-defaults="${PREFIX}"]`);
    const focusId = defaultsPanel?.contains(focused) ? focused.id : null;
    mountHarnessDefaults(sheet, { catalog: offered, prefix: PREFIX, storage: defaultsStorage });
    if (focusId) sheet.querySelector(`#${focusId}`)?.focus();
  }, { owner: opened });
  disposeCatalog = () => catalogRecord.dispose();
  void catalogRecord.read()
    .then(() => {
      // A caller may already hold the catalog, but the sheet still paints it
      // only after it becomes this device's cached record. A later cache write
      // wins over an older promise that settles afterward.
      if (!cacheCatalogSeen && catalog) return catalogRecord.pull(() => Promise.resolve(catalog));
      return undefined;
    })
    .catch(() => {});
}


/** Read what this workspace holds and what its project could give it, then
 *  paint the panel. An answer that lands after the reader moved on writes
 *  nothing: the sheet on screen is somebody else's now. */
function mountDirectories(workspace, { callRpc, current, deviceId, draft, saveDraft }) {
  let detail;
  let project;
  let projectRecord;
  let projectId;
  const paint = () => {
    if (!current()) return;
    const host = $("#wsdirs");
    if (!host || !detail) return;
    const held = new Set((detail.directories || []).map((directory) => directory.source_id));
    const offered = (project?.sources || []).filter((source) => !held.has(source.id));
    const errorText = host.querySelector("#wsdirerr")?.textContent || "";
    if (host.querySelector("#wsdiradd")) draft.directory = directoryDraftOf(host);
    host.innerHTML = directoriesBodyHtml(detail, offered);
    host.querySelector("#wsdirerr").textContent = errorText;
    wireDirectories(workspace, { callRpc, record, current, deviceId, draft, saveDraft });
    restoreDirectoryDraft(host, draft.directory, deviceId);
  };
  const record = watchSettingsRecord(workspaceSettingsAddress(deviceId, workspace.id), (value) => {
    detail = value;
    if (detail?.project_id && detail.project_id !== projectId) {
      projectRecord?.dispose();
      projectId = detail.project_id;
      const sourceProjectId = projectId;
      project = undefined;
      projectRecord = watchSettingsRecord(projectSettingsAddress(deviceId, sourceProjectId), (value) => {
        project = value;
        paint();
      });
      void projectRecord.pull(async () => {
        const listed = await callRpc("project.list");
        return (listed.projects || []).find((candidate) => candidate.project_id === sourceProjectId) || null;
      }).catch(() => {});
    }
    paint();
  });
  void record.pull(() => callRpc("workspace.get", { workspace_id: workspace.id })).catch((thrown) => {
    if (!current() || detail) return;
    const host = $("#wsdirs");
    if (host) host.innerHTML = `<h4>Directories</h4><div class="sub">${esc(thrown.message)}</div>`;
  });
  return () => {
    record.dispose();
    projectRecord?.dispose();
  };
}

function wireDirectories(workspace, { callRpc, record, current, deviceId, draft, saveDraft }) {
  const write = async (method, params, button) => {
    const error = $("#wsdirerr");
    error.textContent = "";
    button.disabled = true;
    try {
      const changed = await callRpc(method, params);
      draft.directory = emptyDirectoryDraft();
      await saveDraft();
      await record.write(changed);
    } catch (thrown) {
      button.disabled = false;
      const visibleError = current() ? $("#wsdirerr") : null;
      if (visibleError) visibleError.textContent = thrown.message;
      else notifyError("The workspace's directories were not changed", thrown.message);
    }
  };
  document.querySelectorAll("#wsdirs [data-remove-directory]").forEach((button) => {
    button.onclick = () =>
      void write(
        "workspace.remove_directory",
        { workspace_id: workspace.id, directory_id: button.dataset.removeDirectory },
        button,
      );
  });
  const choice = $("#wsdiradd");
  const go = $("#wsdiraddgo");
  // The cache readback can replace the inputs inside this persistent holder.
  // Delegation keeps their draft writer live through every restore.
  $("#wsdirfields").oninput = () => {
    draft.directory = directoryDraftOf($("#wsdirs"));
    saveDraft(true);
  };
  choice.onchange = () => {
    $("#wsdirfields").innerHTML = directoryFieldsHtml(choice.value);
    attachRepoPicker($("#wsdirremote"), deviceId);
    go.disabled = !choice.value;
    draft.directory = directoryDraftOf($("#wsdirs"));
    saveDraft();
  };
  restoreDirectoryDraft($("#wsdirs"), draft.directory, deviceId);
  go.onclick = () => {
    const params = addDirectoryParams(choice.value);
    if (!params) {
      $("#wsdirerr").textContent = choice.value === "remote" ? "A Git remote needs a clone url." : "Name the folder to add.";
      return;
    }
    void write("workspace.add_directory", { workspace_id: workspace.id, ...params }, go);
  };
}

/** What the chosen row asks for, or `null` when it is not filled in yet. */
function addDirectoryParams(kind) {
  const name = $("#wsdirlabel")?.value.trim();
  const named = name ? { name } : {};
  if (kind === "path") {
    const path = $("#wsdirpath").value.trim();
    return path ? { path, ...named } : null;
  }
  if (kind === "remote") {
    const remote = $("#wsdirremote").value.trim();
    return remote ? { remote, ...named } : null;
  }
  return kind ? { source_id: kind } : null;
}

/** Save is off until the name actually changed: a Save that does nothing is a
 *  call to a machine and a repaint of every surface that names this workspace,
 *  for no news at all. A blank name is no name, so it never enables either. */
function wireName(workspace, { callRpc, close, onRenamed, draft, saveDraft, clearDraft }) {
  const input = $("#wslabel");
  const save = $("#wssave");
  const changed = () => {
    const value = input.value.trim();
    save.disabled = !value || value === (workspace.name || "").trim();
  };
  input.oninput = () => { draft.name = input.value; saveDraft(true); changed(); };
  save.onclick = () => void rename(workspace, input.value.trim(), { callRpc, close, onRenamed, save, clearDraft });
  return changed;
}

async function rename(workspace, name, { callRpc, close, onRenamed, save, clearDraft }) {
  const error = $("#wserr");
  error.textContent = "";
  save.disabled = true;
  save.textContent = "Saving…";
  try {
    const renamed = await callRpc("workspace.rename", { workspace_id: workspace.id, name });
    await clearDraft();
    close();
    await onRenamed?.(renamed);
  } catch (thrown) {
    save.textContent = "Save name";
    save.disabled = false;
    if (error.isConnected) error.textContent = thrown.message;
    else notifyError("Renaming the workspace failed", thrown.message);
  }
}

/** The confirmation names the workspace and says, in the same words the
 *  bridge acts in, what leaves the disk — the checkouts are the part that
 *  cannot be undone, so they are read before the button is pressed. */
function confirmDeletion(workspace) {
  return confirmAction({
    title: `Delete ${workspace.name || "this workspace"}?`,
    warnings: [
      "This cannot be undone. The checkouts on disk, and any uncommitted or unpushed work in them, will be permanently removed.",
    ],
    actions: [
      "Stop this workspace's agents and terminals.",
      "Remove its checkouts from the repositories they were cut from.",
      "Delete the workspace folder and drop it from Build. The project's own repositories are kept.",
    ],
    confirmLabel: "Delete workspace",
    danger: true,
  });
}

async function deleteWorkspace(workspace, { callRpc, close, onDeleted, storage, deviceId }) {
  const button = $("#wsdelete");
  const error = $("#wserr");
  error.textContent = "";
  button.disabled = true;
  if (!(await confirmDeletion(workspace))) {
    button.disabled = false;
    button.focus();
    return;
  }
  button.textContent = "Deleting…";
  try {
    await callRpc("workspace.delete", { workspace_id: workspace.id });
    await deleteCached([workspaceSettingsAddress(deviceId, workspace.id)]);
  } catch (thrown) {
    button.disabled = false;
    button.textContent = "Delete workspace…";
    if (error.isConnected) error.textContent = thrown.message;
    else notifyError("Deleting the workspace failed", thrown.message);
    return;
  }
  // The id is gone for good, so the preference stored under it is too.
  forgetWorkspaceDefaults(workspace.workspaceKey, storage);
  close();
  try {
    await onDeleted?.(workspace);
  } catch (thrown) {
    notifyError("Workspace deleted, but refreshing the list failed", thrown.message);
  }
}
