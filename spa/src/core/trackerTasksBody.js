// The Tasks tab's body: the one part of the tab that repaints.
//
// Everything above it — the header, the four filters, the Clear press — is
// mounted once and never redrawn (core/trackerPaneChrome.js). What is left is
// this, and it is painted the way the timeline is: by KEY
// (core/patchList.js), so a feed move or a push changes only the rows that
// changed. A row that is still there is still the same element afterwards,
// which is what keeps the press on it pressable, the keyboard where it was,
// and the browser's scroll anchoring holding a node it can still find.
//
// The frame around the rows — the empty line, the board's columns and their
// counts — is patched too (core/domPatch.js), and the lists inside it wear
// `data-keyed-list` so that patch stops at their door: their children belong
// to patchList, and two painters writing one list is one of them undoing the
// other.
//
// Wiring is handed in rather than done here, and it runs once per element that
// had to be MADE. A handler attached on a patch is a handler attached twice.

import { KEYED_LIST_ATTRIBUTE, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { emptyListHtml, taskRowHtml } from "./trackerListRender.js";
import { boardFrameHtml, taskCardHtml } from "./trackerBoardRender.js";
import { esc } from "./text.js";

const keyOf = (task) => task.id;

/**
 * The list's frame.
 *
 * The `<ul>` leads and is always there, empty or not: an empty list that still
 * holds its list is one the next row is INSERTED into rather than one that has
 * to be built. The empty line follows it rather than replacing it — a zero-row
 * list takes no height, so the reader sees the same thing either way, and the
 * patch above never has to tell a `<div>` apart from a `<ul>` in the same slot.
 */
const listFrameHtml = (tasks, filters, paging) =>
  `<ul class="task-rows" ${KEYED_LIST_ATTRIBUTE}></ul>${tasks.length ? "" : emptyListHtml(filters || {})}` +
  (paging?.total > 0
    ? `<div class="task-paging"><span>Showing ${tasks.length} of ${paging.total}</span>${tasks.length < paging.total
      ? '<button class="btn mini" type="button" data-task-more>Load more tasks</button>' : ""}</div>`
    : "");

/** The list view: one keyed row per task. */
export function paintTaskRows(body, tasks, context, wire) {
  patchInnerHtml(body, listFrameHtml(tasks, context.filters, context.paging));
  patchList(body.querySelector(".task-rows"), tasks, {
    keyOf,
    render: (task) => taskRowHtml(task, context),
    wire,
  });
  const more = body.querySelector("[data-task-more]");
  if (more) more.onclick = context.paging.more;
}

/** Groups keep one keyed list each. Collapsing hides rows without discarding
 *  their cached records or changing the order of the next page. */
export function paintGroupedTaskRows(body, groups, context, wire) {
  const loaded = groups.reduce((count, group) => count + group.tasks.length, 0);
  const sections = groups.map((group) => `<section class="task-group" data-task-group="${group.id}">
    <button class="task-group-heading" type="button" data-task-group-toggle="${group.id}" aria-expanded="${!group.collapsed}">
      <span>${esc(group.title)}</span><span class="task-group-count">${group.count}</span>
    </button>
    <ul class="task-rows" ${KEYED_LIST_ATTRIBUTE}${group.collapsed ? " hidden" : ""}></ul>
  </section>`).join("");
  const pager = context.paging?.total > 0
    ? `<div class="task-paging"><span>Showing ${loaded} of ${context.paging.total}</span>${loaded < context.paging.total
      ? '<button class="btn mini" type="button" data-task-more>Load more tasks</button>' : ""}</div>` : "";
  patchInnerHtml(body, sections + (context.paging?.total ? "" : emptyListHtml(context.filters || {})) + pager);
  for (const group of groups) {
    const section = [...body.querySelectorAll("[data-task-group]")]
      .find((element) => element.dataset.taskGroup === group.id);
    patchList(section.querySelector(".task-rows"), group.tasks, {
      keyOf,
      render: (task) => taskRowHtml(task, context),
      wire,
    });
  }
  body.querySelectorAll("[data-task-group-toggle]").forEach((button) => {
    button.onclick = () => context.onToggleGroup(button.dataset.taskGroupToggle);
  });
  const more = body.querySelector("[data-task-more]");
  if (more) more.onclick = context.paging.more;
}

/** Each column's card list, found by the column it drops into rather than by a
 *  selector built out of one: an id is data, and a selector is a language. */
const cardListsIn = (body) =>
  new Map([...body.querySelectorAll("[data-column-drop]")].map((cards) => [cards.dataset.columnDrop, cards]));

/** The board: the columns and their counts are patched, the cards in them are
 *  keyed. A card moved by a drag or an arrow key costs the two columns it
 *  moved between and nothing else on the board. */
export function paintTaskBoard(body, board, context, wire) {
  patchInnerHtml(body, boardFrameHtml(board, context));
  const cardLists = cardListsIn(body);
  for (const column of board) {
    const cards = cardLists.get(column.id);
    if (!cards) continue;
    patchList(cards, column.tasks, { keyOf, render: (task) => taskCardHtml(task, context), wire });
  }
}
