// The rail's projects face, as a pure model: every project the device knows is
// a block, and the block holds the project's live rows — the same rows the
// inbox lists, partitioned and ordered the same way, just gathered under the
// project they belong to. The blocks stand in the inbox's own order too: the
// one whose oldest row has waited longest comes first, and a project with no
// rows at all comes after every one that has some.
//
// A capture nothing has routed yet belongs to no project, so it belongs to no
// block. It stands above them all on its own, an inbox row like any other.
//
// The block's head opens the project's primary checkout — the `main` row is the
// nearest thing a project has to a page — and offers the two creates, a branch
// and an issue, in the split button the rows' Done wears. A block folds shut
// by its chevron and stays that way until it is opened again.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { entryRoute, inboxEntries } from "./inbox.js";

const NO_PROJECT = "";

/** Oldest anchor first; a block nobody can date sorts after the ones somebody
 *  can — the inbox's rule, applied to projects. */
function byAnchor(left, right) {
  if (left.anchorMs === right.anchorMs) return 0;
  if (left.anchorMs === null) return 1;
  if (right.anchorMs === null) return -1;
  return left.anchorMs - right.anchorMs;
}

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

/**
 * The projects face: `{ unsorted, blocks }`.
 *
 * `unsorted` is the rows that belong to no project yet, oldest first. Each
 * block is `{ key, id, name, entries, recent, autoOpen, anchorMs, route,
 * unreadCount }` — its rows partitioned into the list proper and Recent
 * exactly as inboxEntries partitions the inbox, `route` where its head opens
 * (the primary checkout, or nowhere), and the blocks in anchor order.
 */
export function projectBlocks({ items = [], projects = [], nowMs = Date.now() } = {}) {
  const byProject = new Map();
  for (const item of items) {
    const id = item.project_id || NO_PROJECT;
    if (!byProject.has(id)) byProject.set(id, []);
    byProject.get(id).push(item);
  }
  const loose = inboxEntries({ items: byProject.get(NO_PROJECT) || [], nowMs });
  const blocks = [...projectsNamed(projects, items)].map(([id, name]) => {
    const rows = byProject.get(id) || [];
    const partition = inboxEntries({ items: rows, nowMs });
    const all = [...partition.entries, ...partition.recent];
    const dated = all.map((entry) => entry.anchorMs).filter((anchor) => anchor !== null);
    const primary = rows.find((row) => row.kind === "branch" && row.primary);
    return {
      key: `project:${id}`,
      id,
      name,
      entries: partition.entries,
      recent: partition.recent,
      autoOpen: partition.autoOpen,
      anchorMs: dated.length ? Math.min(...dated) : null,
      route: primary ? entryRoute(primary) : null,
      unreadCount: all.reduce((total, entry) => total + entry.unreadCount, 0),
    };
  });
  return { unsorted: [...loose.entries, ...loose.recent], blocks: blocks.sort(byAnchor) };
}

/** The block's create: a branch on the button, an issue behind the caret — the
 *  same split button a finishable row wears, so it reads the same. The menu is
 *  in the markup only while it is open: the DOM patcher leaves a split menu's
 *  `hidden` alone (a poll must not shut what the reader opened), so a menu
 *  that closes has to leave rather than hide. */
function createHtml(block, open) {
  const menu = open
    ? `<div class="splitmenu inbox-menu">
      <div class="mi" data-project-create="${esc(block.id)}" data-create-kind="issue"><span class="mt">New issue…</span><span class="md">Nothing runs until your first message</span></div>
    </div>`
    : "";
  return `<div class="splitbtn inbox-project-create">
      <button class="btn mini" type="button" data-project-create="${esc(block.id)}" data-create-kind="branch" title="New branch in ${esc(block.name)}" aria-label="New branch in ${esc(block.name)}">Branch</button>
      <button class="btn mini caret" type="button" data-menu="${esc(block.key)}" title="More" aria-label="More ways to create in ${esc(block.name)}">▾</button>
      ${menu}
    </div>`;
}

/** The block's head: the fold, the name that opens the project's checkout,
 *  how much inside is waiting, and the create. `ui`: { openMenuKey, folded }. */
export function projectHeadHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.id));
  const unread = block.unreadCount > 0 ? `<span class="badge inbox-unread">${block.unreadCount}</span>` : "";
  const nameClasses = ["inbox-project-name", block.route ? "" : "inbox-unroutable"].filter(Boolean).join(" ");
  const title = block.route ? `Open ${block.name}'s checkout` : `${block.name} has no checkout to open`;
  return `<div class="inbox-project-head">
    <button class="inbox-fold" type="button" data-project-fold="${esc(block.id)}" aria-expanded="${folded ? "false" : "true"}" aria-label="${folded ? "Unfold" : "Fold"} ${esc(block.name)}">${folded ? "▸" : "▾"}</button>
    <button class="${nameClasses}" type="button" data-project-open="${esc(block.id)}" title="${esc(title)}">${esc(block.name)}</button>
    ${unread}
    ${createHtml(block, ui.openMenuKey === block.key)}
  </div>`;
}

/** One block: its head, and the container its rows are reconciled into. The
 *  rows are not rendered here — they are the wiring's keyed list, so a row
 *  keeps its element across paints the way every inbox row does. */
export function projectBlockHtml(block, ui = {}) {
  const folded = !!(ui.folded && ui.folded.has(block.id));
  return `<div class="inbox-project${folded ? " inbox-folded" : ""}" data-key="${esc(block.key)}" data-project="${esc(block.id)}">${projectHeadHtml(
    block,
    ui,
  )}<div class="inbox-project-rows"></div></div>`;
}

/** What a block says when it holds no rows at all. */
export function projectEmptyHtml() {
  return '<div class="inbox-project-empty dim">Nothing here yet.</div>';
}

/** The one control at the foot of the projects face. */
export function newProjectButtonHtml() {
  return '<button class="inbox-new-project" type="button" data-new-project>＋ New project</button>';
}
