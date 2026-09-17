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

/** The defaults panel's own element ids. Distinct from the account page's
 *  `def`, because both panels can be in one document. */
const PREFIX = "wsdef";

const DEFAULTS_BLURB =
  "What a new agent in THIS workspace starts on. Anything left blank falls through to your account-wide defaults in Settings.";

const nameFieldHtml = (name) => `
    <div class="field">
      <label for="wsname">Name</label>
      <input id="wsname" style="width:100%" value="${esc(name || "")}" autocomplete="off" />
      <div class="dim">What this workspace is called in Build. The folder on disk and the branches inside it keep the names they were cut with.</div>
    </div>
    <div class="row">
      <button class="btn" id="wscancel">Close</button>
      <button class="btn primary" id="wssave" style="margin-left:auto" disabled>Save name</button>
    </div>
    <div class="adderr" id="wserr" role="alert"></div>`;

const dangerZoneHtml = () => `
    <section class="field" style="margin-top:24px;border-top:1px solid var(--line);padding-top:16px">
      <h4>Delete workspace</h4>
      <p class="sub">Stop this workspace's agents and terminals, remove its checkouts from disk, and drop it from Build. The project's own repositories are left exactly as they are.</p>
      <button class="btn danger" id="wsdelete">Delete workspace…</button>
    </section>`;

/**
 * Open the workspace settings sheet.
 *
 * `workspace` is one row as the feed names it: `{ id, name, workspaceKey }` —
 * the bare id for the wire, and the account-wide key the browser-local
 * defaults are stored under. `catalog` may be a promise; the defaults panel
 * fills in when that machine answers, and a sheet closed first takes its
 * question with it.
 */
export function openWorkspaceSettings(workspace, { callRpc, catalog, storage = localStorage, onRenamed, onDeleted }) {
  const sheet = $("#sheet");
  sheet.innerHTML = settingsSheetHtml({
    title: "Workspace settings",
    bodyHtml: `${nameFieldHtml(workspace.name)}
      ${harnessDefaultsPanelHtml({ prefix: PREFIX, title: "🤖 Agent defaults here", blurb: DEFAULTS_BLURB })}
      ${dangerZoneHtml()}`,
  });
  $("#scrim").classList.add("show");
  const close = () => $("#scrim").classList.remove("show");
  const opened = sheet.firstElementChild;
  /** Still the sheet this call opened: an answer that lands after the reader
   *  moved on must not write into whatever is on screen now. */
  const current = () => sheet.firstElementChild === opened;

  $("#wscancel").onclick = close;
  wireName(workspace, { callRpc, close, onRenamed });
  $("#wsdelete").onclick = () => void deleteWorkspace(workspace, { callRpc, close, onDeleted, storage });
  Promise.resolve(catalog)
    .then((offered) => {
      if (!current()) return;
      mountHarnessDefaults(sheet, {
        catalog: offered,
        prefix: PREFIX,
        storage: workspaceDefaultsStorage(workspace.workspaceKey, storage),
      });
    })
    .catch(() => {});
}

/** Save is off until the name actually changed: a Save that does nothing is a
 *  call to a machine and a repaint of every surface that names this workspace,
 *  for no news at all. A blank name is no name, so it never enables either. */
function wireName(workspace, { callRpc, close, onRenamed }) {
  const input = $("#wsname");
  const save = $("#wssave");
  const changed = () => {
    const value = input.value.trim();
    save.disabled = !value || value === (workspace.name || "").trim();
  };
  input.oninput = changed;
  save.onclick = () => void rename(workspace, input.value.trim(), { callRpc, close, onRenamed, save });
}

async function rename(workspace, name, { callRpc, close, onRenamed, save }) {
  const error = $("#wserr");
  error.textContent = "";
  save.disabled = true;
  save.textContent = "Saving…";
  try {
    const renamed = await callRpc("workspace.rename", { workspace_id: workspace.id, name });
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

async function deleteWorkspace(workspace, { callRpc, close, onDeleted, storage }) {
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
