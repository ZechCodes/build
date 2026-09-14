// The rail's projects face, as a pure model: the inbox, grouped by project.
// The rows are the inbox's rows in the inbox's order; each is filed under the
// project it belongs to, so a block's rows read top to bottom exactly as they
// do on the inbox, and the blocks stand in the order their first live row
// holds there — the inbox's top row is the top row of the top block. A project
// whose rows have all gone quiet stands after every project with live work,
// and a project with no rows at all after those, in the device's own order.
// Each block partitions its quiet rows into a Recent of its own, by the
// inbox's rule.
//
// A capture nothing has routed yet belongs to no project, so it belongs to no
// block. It stands above them all on its own, an inbox row like any other.
//
// The block's head opens the project's primary checkout — the `main` row is the
// nearest thing a project has to a page — and offers the one create surface
// behind a +. A block folds shut by its chevron and stays that way until it is
// opened again.
//
// A block with nothing live in it is flat: no box, just its head. If it has
// quiet rows they stand straight under the head, folded shut to begin with —
// quiet rows always start hidden — with the chevron unfolding them and no
// Recent disclosure of their own; if it has nothing at all, the chevron has
// nothing to fold and is disabled.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT, ICON_PLUS } from "./icons.js";
import { entryRoute, inboxEntries } from "./inbox.js";
import { projectRoute } from "./projectModel.js";

/** What a project is called on the rail: its name, or the bare id when the
 *  device has given it none. Minted here and read here — the clash set and the
 *  block that looks itself up in it have to agree on the same string. */
const projectNameOf = (project) => project.name || project.id;

/** The blocks' identity and names: every device's projects in the order they
 *  arrived, plus one for any project a row names that its device has not
 *  listed — the row is still work, and it is still somewhere.
 *
 *  Keyed by the account-wide project key, never the bare id: both machines call
 *  their first project `proj-1`, and those are two projects. */
function projectsNamed(projects, items) {
  const named = new Map(projects.map((project) => [project.projectKey, project]));
  for (const item of items) {
    if (!item.projectKey || named.has(item.projectKey)) continue;
    named.set(item.projectKey, {
      id: item.project_id,
      projectKey: item.projectKey,
      deviceId: item.deviceId,
      name: item.project || item.project_id,
    });
  }
  return named;
}

/** The project names more than one device uses. A name that is the account's
 *  own says which project it is; one two machines both use does not, and its
 *  blocks say the device after it. */
export function clashingProjectNames(projects) {
  const devicesByName = new Map();
  for (const project of projects) {
    const name = projectNameOf(project);
    if (!devicesByName.has(name)) devicesByName.set(name, new Set());
    devicesByName.get(name).add(project.deviceId);
  }
  return new Set([...devicesByName].filter(([, devices]) => devices.size > 1).map(([name]) => name));
}

/** The device a project is on, said after its name — only on a name two
 *  devices share, so an account with one device reads exactly as it always has.
 *  Takes anything carrying `{ clash, deviceName }`: the rail's blocks and the
 *  toolbar's menu rows both wear it, and they wear the same tag. */
export function deviceTagHtml(project) {
  if (!project || !project.clash || !project.deviceName) return "";
  return ` <span class="dim">${esc(project.deviceName)}</span>`;
}

/** Where a block stands: by its first live row's place on the inbox; failing
 *  that by its first quiet row's, after every block with live work; failing
 *  that last of all. */
function rankOf(block, liveRank, quietRank) {
  if (liveRank.has(block.projectKey)) return [0, liveRank.get(block.projectKey)];
  if (quietRank.has(block.projectKey)) return [1, quietRank.get(block.projectKey)];
  return [2, 0];
}

const byRank = (left, right) => left.rank[0] - right.rank[0] || left.rank[1] - right.rank[1];

/** The place each project's first row holds in `entries`. */
function firstRank(entries) {
  const rank = new Map();
  entries.forEach((entry, index) => {
    if (entry.projectKey && !rank.has(entry.projectKey)) rank.set(entry.projectKey, index);
  });
  return rank;
}

/** Where a block's head opens: the project's primary checkout, a plain folder's
 *  own surface, or nowhere. */
function blockRoute(project, primary) {
  if (primary) return entryRoute(primary);
  return project.is_git === false ? projectRoute(project) : null;
}

/** One project's block, before it is ranked. `id` stays the bare project id —
 *  that is what every RPC and every create surface wants — while `key`,
 *  `projectKey` and the folds are the account-wide name. */
function blockFor(project, { entries, recent, primary, deviceName, clashes }) {
  const name = projectNameOf(project);
  return {
    key: `project:${project.projectKey}`,
    id: project.id,
    projectKey: project.projectKey,
    deviceId: project.deviceId,
    deviceName,
    clash: clashes.has(name),
    name,
    isGit: project.is_git !== false,
    entries,
    recent,
    flat: entries.length === 0,
    route: blockRoute(project, primary),
    unreadCount: [...entries, ...recent].reduce((total, entry) => total + entry.unreadCount, 0),
  };
}

/**
 * The projects face: `{ unsorted, blocks }`.
 *
 * `unsorted` is the rows that belong to no project yet, in inbox order. Each
 * block is `{ key, id, projectKey, deviceId, deviceName, clash, name, entries,
 * recent, flat, route, unreadCount }` — its rows partitioned into the list
 * proper and Recent exactly as the inbox partitions them, `flat` when nothing
 * in it is live, `route` where its head opens (the primary checkout, or
 * nowhere), and the blocks in the inbox's order.
 *
 * `devices` is the account's device list, which is where a block's device name
 * comes from; it is only ever shown on a name two devices share.
 */
export function projectBlocks({ items = [], projects = [], devices = [], nowMs = Date.now() } = {}) {
  const inbox = inboxEntries({ items, nowMs });
  const liveRank = firstRank(inbox.entries);
  const quietRank = firstRank(inbox.recent);
  const under = (entries, key) => entries.filter((entry) => entry.projectKey === key);
  const unrouted = (entries) => entries.filter((entry) => !entry.projectKey);
  const named = projectsNamed(projects, items);
  const clashes = clashingProjectNames([...named.values()]);
  const deviceNames = new Map(devices.map((device) => [device.id, device.name]));
  const blocks = [...named].map(([key, project]) => {
    const block = blockFor(project, {
      entries: under(inbox.entries, key),
      recent: under(inbox.recent, key),
      primary: items.find((row) => row.kind === "branch" && row.primary && row.projectKey === key),
      deviceName: deviceNames.get(project.deviceId) || null,
      clashes,
    });
    return { ...block, rank: rankOf(block, liveRank, quietRank) };
  });
  return {
    unsorted: [...unrouted(inbox.entries), ...unrouted(inbox.recent)],
    blocks: blocks.sort(byRank).map(({ rank, ...block }) => block),
  };
}

/** Whether a block stands folded: what the user said of it if they have said
 *  anything (`folds`: project key → folded), else shut when all it holds is
 *  quiet rows — those always start hidden — and open otherwise. */
export function blockIsFolded(block, folds) {
  if (folds && folds.has(block.projectKey)) return !!folds.get(block.projectKey);
  return block.flat && block.recent.length > 0;
}

/** The block's head: the fold, the name that opens the project's checkout,
 *  how much inside is waiting, and the + that opens the create surface. The
 *  fold is disabled on a block with nothing to fold. `ui`: { folded } — the
 *  set of folded project keys, as blockIsFolded decides. */
// eslint-disable-next-line complexity -- ratchet: projectHeadHtml is at 12, cap 10 — reduce it, then drop this line
export function projectHeadHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.projectKey));
  const foldable = block.entries.length > 0 || block.recent.length > 0;
  const unread = block.unreadCount > 0 ? `<span class="badge inbox-unread">${block.unreadCount}</span>` : "";
  const nameClasses = ["inbox-project-name", block.route ? "" : "inbox-unroutable"].filter(Boolean).join(" ");
  const title = block.route ? `Open ${block.name}'s checkout` : `${block.name} has no checkout to open`;
  const create = block.isGit
    ? `<button class="iconbtn inbox-project-create" type="button" data-project-create="${esc(block.projectKey)}" title="New branch or issue in ${esc(block.name)}" aria-label="New branch or issue in ${esc(block.name)}">${ICON_PLUS}</button>`
    : "";
  return `<div class="inbox-project-head">
    <button class="iconbtn inbox-fold" type="button" data-project-fold="${esc(block.projectKey)}" aria-expanded="${folded ? "false" : "true"}" aria-label="${folded ? "Unfold" : "Fold"} ${esc(block.name)}"${foldable ? "" : " disabled"}>${folded ? ICON_CHEVRON_RIGHT : ICON_CHEVRON_DOWN}</button>
    <button class="${nameClasses}" type="button" data-project-open="${esc(block.projectKey)}" title="${esc(title)}">${esc(block.name)}${deviceTagHtml(block)}</button>
    ${unread}
    ${create}
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
export function newProjectButtonHtml() {
  return `<button class="inbox-new-project" type="button" data-new-project>${ICON_PLUS}<span>New project</span></button>`;
}
