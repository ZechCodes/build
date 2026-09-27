// The Tasks tab's kanban: one column per column the project has, each card the
// compact form of a list row.
//
// Three rules the markup carries rather than the mount:
//
//   • an empty column is still drawn. A column is not a filter somebody typed
//     — it is what `status` says — so the board is the project's shape and not
//     a summary of what happens to exist.
//   • a card says its state as well as its column. Closing does not move a
//     task to Done, so a closed card in In progress is a real thing and must
//     read as one.
//   • every card is reachable and movable from the keyboard. Dragging is one
//     way to move a card; the arrow keys on a focused card are the other, and
//     neither is the accessible afterthought of the other.
//
// Pure: HTML in, no DOM, no app imports.

import { esc } from "./text.js";
import { actorHref } from "./trackerIdentity.js";
import { KEYED_LIST_ATTRIBUTE } from "./domPatch.js";
import {
  assigneeHtml, labelsHtml, numberHtml, priorityChipHtml, reviewerWords, stateDotHtml, statusChipHtml,
} from "./trackerChips.js";
import { columnNote } from "./trackerModel.js";
import { taskBubbleHtml } from "./taskUnread.js";

/** What the keyboard is told, once per board rather than once per card. */
export const MOVE_HINT = "Use the left and right arrow keys to move this task between columns.";

/**
 * One card.
 *
 * `draggable` and the `data-task`/`data-status` pair are the whole of what the
 * drag needs; `tabindex="0"` and the same pair are the whole of what the
 * keyboard needs. The card is not a link — it holds a link — because a
 * draggable anchor fights the browser's own drag of its href. A watched task
 * with unread wears its bubble at the head's far end (#104).
 */
export const taskCardHtml = (task, { columns, href, unreadOf, ...context }) => {
  const reading = { ...context, identities: task.identities || {} };
  const assigneeHref = task.assignee && actorHref(task.assignee, reading);
  const assignButton = `<button class="task-assign" type="button" data-task-assign="${esc(task.id)}" aria-label="Assign #${esc(String(task.number ?? ""))}" title="Assign this task">${assigneeHref ? "Change" : assigneeHtml(task.assignee, reading)}</button>`;
  // The column already says In review; the card says who it is with (#144).
  const withWhom = reviewerWords(task, reading);
  return (
  `<li class="task-card" draggable="true" tabindex="0"
      data-task="${esc(task.id)}" data-status="${esc(task.status)}"
      aria-label="#${esc(String(task.number ?? ""))} ${esc(task.title)}">
    <div class="task-card-head">${stateDotHtml(task.state)}${numberHtml(task)}${priorityChipHtml(task.priority)}${taskBubbleHtml(task, unreadOf)}</div>
    <a class="task-card-title" href="${esc(href(task))}">${esc(task.title)}</a>
    ${withWhom ? `<div class="task-card-status">${statusChipHtml(columns, task.status, withWhom)}</div>` : ""}
    ${task.labels?.length ? `<div class="task-card-labels">${labelsHtml(task.labels)}</div>` : ""}
    ${assigneeHref
      ? `<span class="task-assignee-entry"><a class="task-assignee-link" href="${esc(assigneeHref)}">${assigneeHtml(task.assignee, reading)}</a>${assignButton}</span>`
      : assignButton}
  </li>`);
};

/**
 * What a column means, said twice over.
 *
 * A `title` for a pointer, and a `details` behind an info glyph for everything
 * that has no pointer — a phone, a keyboard, a screen reader. The same words
 * both ways, from the Tasks Spec (core/trackerModel.js): this board is the
 * one place the user meets rules the agents are told outright and the user
 * never is, and "In review means ready to look at, not accepted" is the whole
 * reason a first-time reader misreads it.
 *
 * `details` rather than a tooltip of our own, as the folds beside it already
 * are: the browser owns the press, so it answers a tap, Enter and a screen
 * reader without a line of wiring.
 */
const columnWhyHtml = (column, note) => `<details class="task-column-why">
      <summary aria-label="What ${esc(column.name)} means" title="${esc(note)}">i</summary>
      <p class="task-column-note" role="note">${esc(note)}</p>
    </details>`;

/**
 * One column, with no cards in it.
 *
 * The count is on the head because a board is read column by column, and "how
 * much is in review" is the question a board is for. The cards themselves are
 * painted into the list by key (core/trackerTasksBody.js), which is what
 * `data-keyed-list` says: this frame is patched on every paint, and the patch
 * stops at the list rather than rebuilding cards that did not change.
 */
const columnHtml = (column, context) => {
  const note = columnNote(context.columns, column.id);
  return `<section class="task-column" data-column="${esc(column.id)}" aria-label="${esc(column.name)}">
    <header class="task-column-head" title="${esc(note)}">
      <h3>${esc(column.name)}</h3>
      ${columnWhyHtml(column, note)}
      <span class="task-column-count">${column.tasks.length}</span>
    </header>
    <ul class="task-column-cards" role="list" data-column-drop="${esc(column.id)}" ${KEYED_LIST_ATTRIBUTE}></ul>
  </section>`;
};

/**
 * The board, as the frame its cards are painted into.
 *
 * It scrolls sideways rather than reflowing: a column that wraps under another
 * is not a column any more, and on a phone the reader would lose the one thing
 * the layout is for. The sheet gives it that scroll under 760px and lets it sit
 * as a row above.
 */
export function boardFrameHtml(board, context) {
  // A group rather than a list: its children are the COLUMNS, and a column is
  // a region with a heading and a list inside it, not a list item.
  return `<div class="task-board" role="group" aria-label="Tasks by column">
    <p class="sr-only">${esc(MOVE_HINT)}</p>
    ${board.map((column) => columnHtml(column, context)).join("")}
  </div>`;
}
