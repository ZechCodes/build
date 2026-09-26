// The rail's projects face, as a pure model: the account's workspaces, grouped
// by the project they are in. A project belongs to one machine, so a block is
// keyed by the account-wide project key and says the machine after its name
// where two machines use that name.
//
// The block's head opens the project's own page — its workspaces, and the agent
// you talk to about the project. The head is that agent's entry (#103): its
// badge is the project agent's unread while the block is open, with the
// project's watched issues no workspace wears (#104), and everything inside
// it, the watched workspace agents' and their issues' too, while it is folded. It offers
// the one create surface behind a +,
// the project's settings, and a ⋯ menu that puts the block away.
// A block folds shut by its chevron and stays that way until it is opened
// again; one with no workspace in it is flat, and its chevron has nothing to
// fold.
// Blocks use the bridge's project session summary, oldest anchor first. The
// summary includes finished workspaces and the project's own conversation.
// More than a day without a message moves a block
// into the projects face's Recent section; workspaces inside keep their own
// session order and Recent partition.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT, ICON_PLUS, ICON_SETTINGS } from "./icons.js";
import {
  PROJECT_AGENT,
  RECENT_AFTER_MS,
  clashingNames,
  dimDeviceHtml,
  menuItemHtml,
  moreButtonHtml,
  railMenuHtml,
  workspaceIsRecent,
} from "./inbox.js";
import { sessionTimes } from "./sessionSpans.js";

/** What a block says instead of its machine's name when that machine cannot be
 *  asked anything: the reader's question about such a block is never "which
 *  laptop" but "why is nothing in here moving". A mark and nothing more — the
 *  block's controls are the same either way. */
export const OFFLINE_TAG = "Offline";

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
 *  and it is still somewhere. Work rows and workspace rows are named the same
 *  way, because they name the same projects.
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
 *  and the project's own page as the block's destination. Every project has one
 *  — the page is about the project, not about anything inside it — so a block is
 *  always routable, however empty it is. */
function workspaceBlockFor(project, rows, tag, nowMs) {
  // The project agent's entry is the block's head, not one of its rows (#103).
  const agentEntry = rows.find((entry) => entry.kind === PROJECT_AGENT) || null;
  const grouped = rows.filter((entry) => entry.kind !== PROJECT_AGENT).sort((left, right) =>
    (left.anchorMs ?? Infinity) - (right.anchorMs ?? Infinity));
  const { anchorMs, lastActivityMs } = sessionTimes(project, nowMs);
  const entries = grouped.filter((entry) => !workspaceIsRecent(entry, nowMs));
  const recent = grouped.filter((entry) => workspaceIsRecent(entry, nowMs));
  const agentUnreadCount = agentEntry?.unreadCount || 0;
  return {
    key: `project:${project.projectKey}`,
    id: project.id,
    projectKey: project.projectKey,
    deviceId: project.deviceId,
    ...tag,
    name: projectNameOf(project),
    isGit: project.is_git !== false,
    entries,
    recent,
    anchorMs,
    lastActivityMs,
    isRecent: lastActivityMs !== null && nowMs - lastActivityMs > RECENT_AFTER_MS,
    flat: entries.length === 0,
    route: { name: "project", projectId: project.id, deviceId: project.deviceId },
    agentEntry,
    // The head's badge, by the fold (#103): open, the project agent's unread
    // alone — the rows under it wear their own; folded, everything the block
    // is holding: the project agent's and every watched workspace agent's.
    agentUnreadCount,
    unreadCount: grouped.reduce((total, entry) => total + entry.unreadCount, agentUnreadCount),
  };
}

/** The count a block's head wears: the project agent's alone while the block
 *  is open, everything inside it while it is folded. */
export const headUnreadCount = (block, folded) => (folded ? block.unreadCount : block.agentUnreadCount);

/** Group the landing rail's workspace rows by the project they are in. A
 *  project belongs to one machine, so the grouping is by the account-wide
 *  project key and never by a name or a bare id: two machines each mint a
 *  `proj-1`, and two projects may share a name — which is what the device tag
 *  on the head is for. */
export function workspaceProjectBlocks(entries = [], projects = [], devices = [], offlineDeviceIds = null, nowMs = Date.now()) {
  const named = projectsNamed(projects, entries);
  const tags = deviceTags([...named.values()], devices, offlineDeviceIds);
  const blocks = [...named.values()].map((project) =>
    workspaceBlockFor(
      project,
      entries.filter((entry) => entry.projectKey === project.projectKey),
      tags.get(project.projectKey),
      nowMs,
    ),
  ).sort((left, right) =>
    (left.anchorMs === null) - (right.anchorMs === null)
      || (left.anchorMs === null ? 0 : left.anchorMs - right.anchorMs)
      || left.name.localeCompare(right.name, undefined, { sensitivity: "base" })
      || left.projectKey.localeCompare(right.projectKey),
  );
  return {
    unsorted: entries.filter((entry) => !entry.projectKey),
    blocks: blocks.filter((block) => !block.isRecent),
    recentBlocks: blocks.filter((block) => block.isRecent),
  };
}


/** The project names more than one device uses. A name that is the account's
 *  own says which project it is; one two machines both use does not, and its
 *  blocks say the device after it. */
const clashingProjectNames = (projects) => clashingNames(projects, projectNameOf);

/** What each project in a set wears to say which device it is on, by project
 *  key: the `{ clash, deviceName, offline }` deviceTagHtml reads. Whether a name
 *  needs its device said is a fact about the whole set, so the set is asked once
 *  and every project reads itself out of that one answer. The rail's blocks and
 *  the toolbar's menu rows are both minted here, so they wear the same tag.
 *
 *  `offlineDeviceIds` is the set of machines that cannot be asked anything right
 *  now, and null means the caller is not asking about that at all — the toolbar
 *  lists projects to go to, not machines to worry about, so its rows are marked
 *  exactly as they always were. A project whose machine the account's device
 *  list has never heard of is offline too: nothing can answer for it. */
export function deviceTags(projects, devices = [], offlineDeviceIds = null) {
  const clashes = clashingProjectNames(projects);
  const known = new Map(devices.map((device) => [device.id, device]));
  return new Map(
    projects.map((project) => [
      project.projectKey,
      {
        clash: clashes.has(projectNameOf(project)),
        deviceName: known.get(project.deviceId)?.name || null,
        offline: offlineDeviceIds
          ? !known.has(project.deviceId) || offlineDeviceIds.has(project.deviceId)
          : false,
      },
    ]),
  );
}

/** The device a project is on, said after its name — only on a name two
 *  devices share, so an account with one device reads exactly as it always has.
 *  Takes anything carrying `{ clash, deviceName, offline }`: the rail's blocks
 *  and the toolbar's menu rows both wear it, and they wear the same tag.
 *
 *  A project whose machine is away says so ALWAYS, clash or no clash. Which
 *  laptop holds it stops being the useful fact the moment none of them can
 *  answer: what the reader needs to know is why nothing in the block is
 *  moving, and "Offline" is that in one word. */
export function deviceTagHtml(project) {
  if (!project) return "";
  if (project.offline) return dimDeviceHtml(OFFLINE_TAG);
  if (!project.clash) return "";
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

/** The block's ⋯: one quiet control beside Settings and the +, named the way a
 *  row's is, because it opens the block's menu the way a row's opens the row's. */
const blockMoreHtml = (block, open) => moreButtonHtml(block.key, block.name, open);

/**
 * The block's menu, in the markup only while it is open — the rows' rule, for
 * the rows' reason: the DOM patcher leaves a split menu's `hidden` alone.
 *
 * Its one item puts the block away. A machine that has gone leaves its projects
 * on the rail for ever: nothing can refresh them, nothing can be done in them,
 * and an account that has retired a laptop reads its blocks every day for work
 * it will never pick up again. So a block can be dropped — from the cache,
 * which is all it is once its machine has gone (core/projectHide.js).
 *
 * It is hide, not delete: nothing on the machine is touched, and the project
 * comes back the moment that machine lists it again. It is offered on every
 * block, whatever this tab knows about the machine right now: which controls a
 * block has is what the cache holds, never whether a session has landed yet. On
 * a machine that is answering it simply comes back on the next pass.
 */
const blockMenuHtml = (block, open) => railMenuHtml(open, `Actions for ${block.name}`, [
  menuItemHtml(`data-project-hide="${esc(block.projectKey)}"`, "Hide project", "Takes it off the rail until its machine lists it again"),
]);

/** The block's head: the fold, the name that opens the project's own page,
 *  how much inside is waiting, the project's settings, the + that starts
 *  another workspace, and the ⋯ behind which the block's menu opens. The fold
 *  is disabled on a block with nothing to fold. `ui`: { folded, openMenuKey } —
 *  the set of folded project keys, as blockIsFolded decides, and the one menu
 *  the rail holds open. The menu hangs off the head itself rather than the
 *  cluster its ⋯ sits in, so the grey an away block's head wears never reaches
 *  it (styles/shell.css). */
export function projectHeadHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.projectKey));
  const menuOpen = ui.openMenuKey === block.key;
  const count = headUnreadCount(block, folded);
  const unread = count > 0 ? `<span class="badge inbox-unread">${count}</span>` : "";
  const title = `Open ${block.name}`;
  const create = `<button class="iconbtn inbox-project-create" type="button" data-project-create="${esc(block.projectKey)}" aria-label="New workspace in ${esc(block.name)}" title="New workspace in ${esc(block.name)}">${ICON_PLUS}</button>`;
  const device = deviceTagHtml(block);
  return `<div class="inbox-project-head">
    ${foldButtonHtml(block, folded)}
    <button class="inbox-project-name" type="button" data-project-open="${esc(block.projectKey)}" title="${esc(title)}">${esc(block.name)}</button>
    <span class="inbox-project-tools"><span class="inbox-project-device">${device}</span><span class="inbox-project-actions">${blockMoreHtml(block, menuOpen)}<button class="iconbtn inbox-project-settings" type="button" data-project-settings="${esc(block.projectKey)}" aria-label="Settings for ${esc(block.name)}" title="Settings for ${esc(block.name)}">${ICON_SETTINGS}</button>${create}</span></span>
    ${unread}${blockMenuHtml(block, menuOpen)}
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
