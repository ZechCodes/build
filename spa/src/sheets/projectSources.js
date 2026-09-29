// The Sources section of Project settings: one card per folder a project cuts
// its workspaces from, each holding what that folder is — its label, where it
// stands, the branch new work starts from, and the remote its checkout pushes
// to — with its own Save and Remove.
//
// A project has no remote of its own (#228). Each source's is its checkout's
// `origin`, which the bridge reads back from the checkout for every row, so a
// card paints what the checkout holds, not what was typed into it once.

import { esc } from "../core/text.js";
import { readCached } from "../core/localCache.js";
import { workspaceDisplayName } from "../core/workspaceModel.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { attachRepoPicker } from "./repoPicker.js";
import { mountSyncControls, syncControlsHtml } from "./sourceSyncControls.js";

/** The parts of a source a card shows, in the order it shows them. */
const FIELDS = ["name", "path", "base_branch", "remote"];

/** Which parts only a Git repository has. */
const GIT_FIELDS = new Set(["base_branch", "remote"]);

const LABELS = { name: "Label", path: "Folder", base_branch: "Base branch", remote: "Remote (origin)" };

const PLACEHOLDERS = { remote: "git@github.com:org/repo.git", base_branch: "main" };

/** What the card shows for a part of a source, as the bridge last said it. */
export const paintedValue = (source, field) =>
  field === "name" ? source.name || source.mount || "" : source[field] || "";

const fieldId = (field, index) => `ps-${field.replace("_", "")}-${index}`;

/** How the sheet edits a source on a bridge that serves
 *  `project.update_source`: every part of every source, bar the first
 *  source's folder, which is the project's home. */
const IN_PLACE = {
  editable: (field, index) => field !== "path" || index > 0,
  save: (callRpc, projectId, source, changes) =>
    callRpc("project.update_source", { project_id: projectId, source_id: source.id, ...changes }),
};

/** On an older bridge the one edit is the first source's remote, which is
 *  what `project.set_remote` writes. */
const FIRST_REMOTE_ONLY = {
  editable: (field, index) => field === "remote" && index === 0,
  save: (callRpc, projectId, _source, changes) =>
    callRpc("project.set_remote", { project_id: projectId, url: changes.remote }),
};

export const sourceEditing = (editsSources) => (editsSources ? IN_PLACE : FIRST_REMOTE_ONLY);

const shownFields = (source) => FIELDS.filter((field) => source.is_git !== false || !GIT_FIELDS.has(field));

function fieldHtml(source, index, field, editing) {
  const id = fieldId(field, index);
  const editable = editing.editable(field, index);
  const placeholder = PLACEHOLDERS[field] ? ` placeholder="${PLACEHOLDERS[field]}"` : "";
  const hint = field === "path" && index === 0
    ? '<div class="dim ps-hint">The first folder is the project\'s home and cannot move.</div>'
    : "";
  return `<div class="field"><label for="${id}">${LABELS[field]}</label>
      <input id="${id}" data-field="${field}" style="width:100%" ${fieldTraits("identifier")}${placeholder}
        value="${esc(paintedValue(source, field))}"${editable ? "" : " readonly"}>${hint}</div>`;
}

function cardHtml(source, index, { editing, removable, notice, sync }) {
  const kind = source.is_git === false ? "Folder" : "Git repository";
  const editable = shownFields(source).some((field) => editing.editable(field, index));
  return `<article class="ps-source" data-source-id="${esc(source.id)}" data-source-index="${index}">
    <header class="ps-source-head"><span class="ps-source-title">${esc(paintedValue(source, "name") || `Folder ${index + 1}`)}</span>
      <span class="ps-source-tag">${kind}</span></header>
    ${shownFields(source).map((field) => fieldHtml(source, index, field, editing)).join("")}
    ${syncControlsHtml(source, index, sync)}
    <div class="adderr" data-source-error role="alert"></div>
    <div class="dim" data-source-status role="status">${esc(notice?.status || "")}</div>
    <div class="ps-source-warning" data-source-warning role="alert">${esc(notice?.warning || "")}</div>
    <footer class="row ps-source-foot">${[
      removable ? `<button class="btn danger mini" type="button" data-remove-source="${esc(source.id)}">Remove</button>` : "",
      editable ? `<button class="btn primary mini" type="button" data-save-source="${esc(source.id)}" disabled>Save</button>` : "",
    ].join("")}</footer></article>`;
}

/** The section: a card per source, then the two ways one is added. */
export function sourcesSectionHtml(project, { editing, editsSources, notices = {}, sync = {} }) {
  const sources = project.sources || [];
  const removable = sources.length > 1;
  const older = editsSources
    ? ""
    : '<p class="dim">Update Build on this device to edit a folder\'s label, path and base branch here.</p>';
  return `<section class="ps-section" aria-labelledby="ps-sources-h"><h4 id="ps-sources-h">Sources</h4>
    <p class="sub">New workspaces are cut from these folders. A workspace that already exists keeps its own directories.</p>
    ${older}
    ${sources.map((source, index) => cardHtml(source, index, { editing, removable, notice: notices[source.id], sync })).join("")}
    <div class="row"><button class="btn" id="psaddfolder" type="button">Add folder…</button>
      <button class="btn" id="psaddremote" type="button">Add Git remote…</button></div>
    <div id="psaddsource"></div>
    <div class="adderr" id="pssrcerr" role="alert"></div></section>`;
}

const cardOf = (sheet, sourceId) =>
  [...sheet.querySelectorAll(".ps-source")].find((card) => card.dataset.sourceId === sourceId);

const sourceOf = (project, card) => (project.sources || [])[Number(card.dataset.sourceIndex)];

/** The parts of one card that differ from what the bridge last said. */
function cardChanges(card, source) {
  const changes = {};
  card.querySelectorAll("input[data-field]:not([readonly])").forEach((input) => {
    const value = input.value.trim();
    if (value !== paintedValue(source, input.dataset.field)) changes[input.dataset.field] = value;
  });
  return changes;
}

/** Every card's unsaved edits, by source id. Cards with none are left out. */
export function readSourceEdits(sheet, project) {
  const edits = {};
  sheet.querySelectorAll(".ps-source").forEach((card) => {
    const source = sourceOf(project, card);
    const changes = source ? cardChanges(card, source) : {};
    if (Object.keys(changes).length) edits[card.dataset.sourceId] = changes;
  });
  return edits;
}

function markDirty(card, source) {
  const save = card.querySelector("[data-save-source]");
  if (save) save.disabled = !Object.keys(cardChanges(card, source)).length;
}

/** Put unsaved edits back into the cards they were typed into. */
export function restoreSourceEdits(sheet, project, edits = {}) {
  Object.entries(edits).forEach(([sourceId, changes]) => {
    const card = cardOf(sheet, sourceId);
    if (!card) return;
    Object.entries(changes).filter(([field]) => FIELDS.includes(field)).forEach(([field, value]) => {
      const input = card.querySelector(`input[data-field="${field}"]:not([readonly])`);
      if (input) input.value = value;
    });
    markDirty(card, sourceOf(project, card));
  });
}

/** What a save says once it lands: how many existing workspace checkouts
 *  followed the remote, when any did, and — apart, as a warning — which
 *  workspaces Git could not move and so still use the old one. A workspace
 *  this device's cache knows is named; one it does not is named by its path. */
export function savedNotice(changed, workspaces = []) {
  const moved = Number(changed?.checkouts_updated) || 0;
  const status = moved
    ? `Saved. ${moved} existing workspace checkout${moved === 1 ? "" : "s"} now use${moved === 1 ? "s" : ""} the new remote.`
    : "Saved.";
  return { status, warning: leftBehindWarning(changed?.checkouts_failed, workspaces) };
}

function leftBehindWarning(failed, workspaces) {
  if (!Array.isArray(failed) || !failed.length) return "";
  const nameOf = (left) => {
    const workspace = workspaces.find((candidate) => (candidate?.workspace_id || candidate?.id) === left.workspace_id);
    return workspace ? workspaceDisplayName(workspace, left.path) : left.path;
  };
  const count = failed.length === 1 ? "1 existing workspace still uses" : `${failed.length} existing workspaces still use`;
  return `${count} the old remote because Git could not change it there: ${failed.map(nameOf).join(", ")}.`;
}

async function cachedWorkspaces(deviceId) {
  const record = await readCached({ deviceId, entityId: "", kind: "workspaces" });
  return Array.isArray(record?.value) ? record.value : [];
}

/**
 * Wire every card: typing marks it dirty and keeps the draft, Save sends only
 * what changed through `editing.save`, Remove takes the source off. Each
 * write answers the project row, which `write(send, button, errorElement,
 * onDone)` puts in the cache; the sheet repaints from there once `onDone`
 * has settled.
 */
export function mountSourceCards(sheet, project, { editing, callRpc, deviceId, write, saveDraft, onSaved, onSyncAsked }) {
  sheet.querySelectorAll(".ps-source").forEach((card) => {
    const source = sourceOf(project, card);
    if (!source) return;
    mountSyncControls(card, source, { callRpc, projectId: project.project_id, write, onSyncAsked });
    card.querySelectorAll("input[data-field]:not([readonly])").forEach((input) => {
      input.oninput = () => { markDirty(card, source); saveDraft(true); };
    });
    const remote = card.querySelector('input[data-field="remote"]:not([readonly])');
    if (remote) attachRepoPicker(remote, deviceId);
    const save = card.querySelector("[data-save-source]");
    if (save) save.onclick = () => void saveCard(card, source, { editing, callRpc, deviceId, project, write, onSaved });
  });
  sheet.querySelectorAll("[data-remove-source]").forEach((button) => {
    button.onclick = () => void write(
      () => callRpc("project.remove_source", { project_id: project.project_id, source_id: button.dataset.removeSource }),
      button,
      sheet.querySelector("#pssrcerr"),
    );
  });
}

async function saveCard(card, source, { editing, callRpc, deviceId, project, write, onSaved }) {
  const changes = cardChanges(card, source);
  const error = card.querySelector("[data-source-error]");
  await write(
    () => editing.save(callRpc, project.project_id, source, changes),
    card.querySelector("[data-save-source]"),
    error,
    async (changed) => onSaved(source.id, savedNotice(changed, await cachedWorkspaces(deviceId))),
  );
}
