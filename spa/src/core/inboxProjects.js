// The rail's projects face, as a pure model: the account's workspaces, grouped
// by the project they are in. A project belongs to one machine, so a block is
// keyed by the account-wide project key and says the machine after its name
// where two machines use that name.
//
// The block's head opens the workspace the route is standing in — failing that
// the first one the project holds — and offers the one create surface behind a
// +. A block folds shut by its chevron and stays that way until it is opened
// again; one with no workspace in it is flat, and its chevron has nothing to
// fold.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT, ICON_PLUS, ICON_SETTINGS } from "./icons.js";
import { clashingNames, dimDeviceHtml } from "./inbox.js";

/** What a project is called: its name, or the bare id when the device has
 *  given it none. Minted here and read everywhere — the clash set, the block
 *  and the toolbar's menu row all have to agree on the same string. */
export const projectNameOf = (project) => project.name || project.id;

/** The bare project id a row names, whichever half of the app it came from: a
 *  feed row carries the wire's `project_id`, a rail entry the `projectId` the
 *  inbox normalized it to. Read here so the rest of this module never asks. */
const bareProjectIdOf = (row) => row.project_id || row.projectId || "";

/** The blocks' identity and names: every device's projects, plus one for any
 *  project a row names that its device has not listed — the row is still work,
 *  and it is still somewhere. The final blocks are ordered by name after every
 *  device is merged, so a machine's arrival order never partitions the rail.
 *  Work rows and
 *  workspace rows are named the same way, because they name the same projects.
 *
 *  Keyed by the account-wide project key, never the bare id: both machines call
 *  their first project `proj-1`, and those are two projects. */
function projectsNamed(projects, rows) {
  const named = new Map(projects.map((project) => [project.projectKey, project]));
  for (const row of rows) {
    if (!row.projectKey || named.has(row.projectKey)) continue;
    const id = bareProjectIdOf(row);
    named.set(row.projectKey, { id, projectKey: row.projectKey, deviceId: row.deviceId, name: row.project || id });
  }
  return named;
}

/** One project's block on the landing rail's workspace face: its workspaces,
 *  and the one the route is standing in as the block's own destination. */
function workspaceBlockFor(project, grouped, tag, activeWorkspaceKey) {
  const standing = grouped.find((entry) => entry.workspaceKey === activeWorkspaceKey && entry.route);
  return {
    key: `project:${project.projectKey}`,
    id: project.id,
    projectKey: project.projectKey,
    deviceId: project.deviceId,
    ...tag,
    name: projectNameOf(project),
    isGit: project.is_git !== false,
    entries: grouped,
    recent: [],
    flat: grouped.length === 0,
    route: (standing || grouped.find((entry) => entry.route))?.route || null,
    unreadCount: grouped.reduce((total, entry) => total + entry.unreadCount, 0),
  };
}

/** Group the landing rail's workspace rows by the project they are in. A
 *  project belongs to one machine, so the grouping is by the account-wide
 *  project key and never by a name or a bare id: two machines each mint a
 *  `proj-1`, and two projects may share a name — which is what the device tag
 *  on the head is for. */
export function workspaceProjectBlocks(entries = [], projects = [], activeWorkspaceKey = null, devices = []) {
  const named = projectsNamed(projects, entries);
  const tags = deviceTags([...named.values()], devices);
  const blocks = [...named.values()].sort((left, right) =>
    projectNameOf(left).localeCompare(projectNameOf(right), undefined, { sensitivity: "base" })
      || left.projectKey.localeCompare(right.projectKey),
  ).map((project) =>
    workspaceBlockFor(
      project,
      entries.filter((entry) => entry.projectKey === project.projectKey),
      tags.get(project.projectKey),
      activeWorkspaceKey,
    ),
  );
  return { unsorted: entries.filter((entry) => !entry.projectKey), blocks };
}


/** The project names more than one device uses. A name that is the account's
 *  own says which project it is; one two machines both use does not, and its
 *  blocks say the device after it. */
const clashingProjectNames = (projects) => clashingNames(projects, projectNameOf);

/** What each project in a set wears to say which device it is on, by project
 *  key: the `{ clash, deviceName }` deviceTagHtml reads. Whether a name needs
 *  its device said is a fact about the whole set, so the set is asked once and
 *  every project reads itself out of that one answer. The rail's blocks and the
 *  toolbar's menu rows are both minted here, so they wear the same tag. */
export function deviceTags(projects, devices = []) {
  const clashes = clashingProjectNames(projects);
  const deviceNames = new Map(devices.map((device) => [device.id, device.name]));
  return new Map(
    projects.map((project) => [
      project.projectKey,
      { clash: clashes.has(projectNameOf(project)), deviceName: deviceNames.get(project.deviceId) || null },
    ]),
  );
}

/** The device a project is on, said after its name — only on a name two
 *  devices share, so an account with one device reads exactly as it always has.
 *  Takes anything carrying `{ clash, deviceName }`: the rail's blocks and the
 *  toolbar's menu rows both wear it, and they wear the same tag. */
export function deviceTagHtml(project) {
  if (!project || !project.clash) return "";
  return dimDeviceHtml(project.deviceName);
}

/**
 * Which machine each project's rows are to say they are on, by project key —
 * null where the name says which project it is on its own.
 *
 * Built from the same set of projects the blocks are, and by the same rule, so
 * a row and the block it sits in never disagree about whether a name needs its
 * machine said. Asked once for a whole list, so the row painters are handed the
 * answer rather than deciding it a row at a time.
 */
export function rowDeviceNames({ items = [], projects = [], devices = [] } = {}) {
  const tags = deviceTags([...projectsNamed(projects, items).values()], devices);
  return new Map([...tags].map(([key, tag]) => [key, tag.clash ? tag.deviceName : null]));
}

/** Whether a block stands folded: what the user said of it if they have said
 *  anything (`folds`: project key → folded), else shut when all it holds is
 *  quiet rows — those always start hidden — and open otherwise. */
export function blockIsFolded(block, folds) {
  if (folds && folds.has(block.projectKey)) return !!folds.get(block.projectKey);
  return block.flat && block.recent.length > 0;
}

/** The chevron that folds a block shut and unfolds it again, saying which of
 *  those a press would do. A block with nothing inside it has nothing to fold,
 *  so its chevron is disabled rather than absent: the heads stay in line. */
function foldButtonHtml(block, folded) {
  const foldable = block.entries.length > 0 || block.recent.length > 0;
  return `<button class="iconbtn inbox-fold" type="button" data-project-fold="${esc(block.projectKey)}" aria-expanded="${folded ? "false" : "true"}" aria-label="${folded ? "Unfold" : "Fold"} ${esc(block.name)}"${foldable ? "" : " disabled"}>${folded ? ICON_CHEVRON_RIGHT : ICON_CHEVRON_DOWN}</button>`;
}

/** The block's head: the fold, the name that opens the project's workspace,
 *  how much inside is waiting, and the + that starts another one. The fold is
 *  disabled on a block with nothing to fold. `ui`: { folded } — the set of
 *  folded project keys, as blockIsFolded decides. */
export function projectHeadHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.projectKey));
  const unread = block.unreadCount > 0 ? `<span class="badge inbox-unread">${block.unreadCount}</span>` : "";
  const nameClasses = ["inbox-project-name", block.route ? "" : "inbox-unroutable"].filter(Boolean).join(" ");
  const title = block.route ? `Open ${block.name}'s workspace` : `${block.name} has no workspace to open`;
  const create = `<button class="iconbtn inbox-project-create" type="button" data-project-create="${esc(block.projectKey)}" aria-label="New workspace in ${esc(block.name)}" title="New workspace in ${esc(block.name)}">${ICON_PLUS}</button>`;
  const device = deviceTagHtml(block);
  return `<div class="inbox-project-head">
    ${foldButtonHtml(block, folded)}
    <button class="${nameClasses}" type="button" data-project-open="${esc(block.projectKey)}" title="${esc(title)}">${esc(block.name)}</button>
    <span class="inbox-project-tools"><span class="inbox-project-device">${device}</span><span class="inbox-project-actions"><button class="iconbtn inbox-project-settings" type="button" data-project-settings="${esc(block.projectKey)}" aria-label="Settings for ${esc(block.name)}" title="Settings for ${esc(block.name)}">${ICON_SETTINGS}</button>${create}</span></span>
    ${unread}
  </div>`;
}

/** One block: its head, and the container its rows are reconciled into. The
 *  rows are not rendered here — they are the wiring's keyed list, so a row
 *  keeps its element across paints the way every inbox row does. `ui`:
 *  { folded, activeProjectId } — the active block is the one holding the
 *  branch or issue the route stands on. */
export function projectBlockHtml(block, ui = {}) {
  const classes = [
    "inbox-project",
    block.flat ? "inbox-flat" : "",
    ui.folded && ui.folded.has(block.projectKey) ? "inbox-folded" : "",
    ui.activeProjectId === block.projectKey ? "active" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return `<div class="${classes}" data-key="${esc(block.key)}" data-project="${esc(block.projectKey)}">${projectHeadHtml(
    block,
    ui,
  )}<div class="inbox-project-rows"></div></div>`;
}

/** The one control at the head of the projects face. */
