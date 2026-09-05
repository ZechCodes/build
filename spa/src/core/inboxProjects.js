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

/** The blocks' identity and names: the device's projects in its order, plus
 *  one for any project a row names that the device has not listed — the row
 *  is still work, and it is still somewhere. */
function projectsNamed(projects, items) {
  const named = new Map(projects.map((project) => [project.id, project.name || project.id]));
  for (const item of items) {
    if (!item.project_id || named.has(item.project_id)) continue;
    named.set(item.project_id, item.project || item.project_id);
  }
  return named;
}

/** Where a block stands: by its first live row's place on the inbox; failing
 *  that by its first quiet row's, after every block with live work; failing
 *  that last of all. */
function rankOf(block, liveRank, quietRank) {
  if (liveRank.has(block.id)) return [0, liveRank.get(block.id)];
  if (quietRank.has(block.id)) return [1, quietRank.get(block.id)];
  return [2, 0];
}

const byRank = (left, right) => left.rank[0] - right.rank[0] || left.rank[1] - right.rank[1];

/** The place each project's first row holds in `entries`. */
function firstRank(entries) {
  const rank = new Map();
  entries.forEach((entry, index) => {
    if (entry.projectId && !rank.has(entry.projectId)) rank.set(entry.projectId, index);
  });
  return rank;
}

/**
 * The projects face: `{ unsorted, blocks }`.
 *
 * `unsorted` is the rows that belong to no project yet, in inbox order. Each
 * block is `{ key, id, name, entries, recent, flat, route, unreadCount }` —
 * its rows partitioned into the list proper and Recent exactly as the inbox
 * partitions them, `flat` when nothing in it is live,
 * `route` where its head opens (the primary checkout, or nowhere), and the
 * blocks in the inbox's order.
 */
export function projectBlocks({ items = [], projects = [], nowMs = Date.now() } = {}) {
  const inbox = inboxEntries({ items, nowMs });
  const liveRank = firstRank(inbox.entries);
  const quietRank = firstRank(inbox.recent);
  const under = (entries, id) => entries.filter((entry) => entry.projectId === id);
  const blocks = [...projectsNamed(projects, items)].map(([id, name]) => {
    const entries = under(inbox.entries, id);
    const recent = under(inbox.recent, id);
    const primary = items.find((row) => row.kind === "branch" && row.primary && row.project_id === id);
    const block = {
      key: `project:${id}`,
      id,
      name,
      entries,
      recent,
      flat: entries.length === 0,
      route: primary ? entryRoute(primary) : null,
      unreadCount: [...entries, ...recent].reduce((total, entry) => total + entry.unreadCount, 0),
    };
    return { ...block, rank: rankOf(block, liveRank, quietRank) };
  });
  return {
    unsorted: [...under(inbox.entries, ""), ...under(inbox.recent, "")],
    blocks: blocks.sort(byRank).map(({ rank, ...block }) => block),
  };
}

/** Whether a block stands folded: what the user said of it if they have said
 *  anything (`folds`: project id → folded), else shut when all it holds is
 *  quiet rows — those always start hidden — and open otherwise. */
export function blockIsFolded(block, folds) {
  if (folds && folds.has(block.id)) return !!folds.get(block.id);
  return block.flat && block.recent.length > 0;
}

/** The block's head: the fold, the name that opens the project's checkout,
 *  how much inside is waiting, and the + that opens the create surface. The
 *  fold is disabled on a block with nothing to fold. `ui`: { folded } — the
 *  set of folded project ids, as blockIsFolded decides. */
export function projectHeadHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.id));
  const foldable = block.entries.length > 0 || block.recent.length > 0;
  const unread = block.unreadCount > 0 ? `<span class="badge inbox-unread">${block.unreadCount}</span>` : "";
  const nameClasses = ["inbox-project-name", block.route ? "" : "inbox-unroutable"].filter(Boolean).join(" ");
  const title = block.route ? `Open ${block.name}'s checkout` : `${block.name} has no checkout to open`;
  return `<div class="inbox-project-head">
    <button class="iconbtn inbox-fold" type="button" data-project-fold="${esc(block.id)}" aria-expanded="${folded ? "false" : "true"}" aria-label="${folded ? "Unfold" : "Fold"} ${esc(block.name)}"${foldable ? "" : " disabled"}>${folded ? ICON_CHEVRON_RIGHT : ICON_CHEVRON_DOWN}</button>
    <button class="${nameClasses}" type="button" data-project-open="${esc(block.id)}" title="${esc(title)}">${esc(block.name)}</button>
    ${unread}
    <button class="iconbtn inbox-project-create" type="button" data-project-create="${esc(block.id)}" title="New branch or issue in ${esc(block.name)}" aria-label="New branch or issue in ${esc(block.name)}">${ICON_PLUS}</button>
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
    ui.folded && ui.folded.has(block.id) ? "inbox-folded" : "",
    ui.activeProjectId === block.id ? "active" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return `<div class="${classes}" data-key="${esc(block.key)}" data-project="${esc(block.id)}">${projectHeadHtml(
    block,
    ui,
  )}<div class="inbox-project-rows"></div></div>`;
}

/** The one control at the head of the projects face. */
export function newProjectButtonHtml() {
  return `<button class="inbox-new-project" type="button" data-new-project>${ICON_PLUS}<span>New project</span></button>`;
}
