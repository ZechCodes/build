// The Issues tab's kanban: one column per column the project has, each card the
// compact form of a list row.
//
// Three rules the markup carries rather than the mount:
//
//   • an empty column is still drawn. A column is not a filter somebody typed
//     — it is what `status` says — so the board is the project's shape and not
//     a summary of what happens to exist.
//   • a card says its state as well as its column. Closing does not move an
//     issue to Done, so a closed card in In progress is a real thing and must
//     read as one.
//   • every card is reachable and movable from the keyboard. Dragging is one
//     way to move a card; the arrow keys on a focused card are the other, and
//     neither is the accessible afterthought of the other.
//
// Pure: HTML in, no DOM, no app imports.

import { esc } from "./text.js";
import { assigneeHtml, labelsHtml, numberHtml, priorityChipHtml, stateDotHtml } from "./trackerChips.js";

/** What the keyboard is told, once per board rather than once per card. */
export const MOVE_HINT = "Use the left and right arrow keys to move this issue between columns.";

/**
 * One card.
 *
 * `draggable` and the `data-issue`/`data-status` pair are the whole of what the
 * drag needs; `tabindex="0"` and the same pair are the whole of what the
 * keyboard needs. The card is not a link — it holds a link — because a
 * draggable anchor fights the browser's own drag of its href.
 */
export const issueCardHtml = (issue, { columns, agentLabels, href }) =>
  `<li class="issue-card" draggable="true" tabindex="0"
      data-issue="${esc(issue.id)}" data-status="${esc(issue.status)}"
      aria-label="#${esc(String(issue.number ?? ""))} ${esc(issue.title)}">
    <div class="issue-card-head">${stateDotHtml(issue.state)}${numberHtml(issue)}${priorityChipHtml(issue.priority)}</div>
    <a class="issue-card-title" href="${esc(href(issue))}">${esc(issue.title)}</a>
    ${issue.labels?.length ? `<div class="issue-card-labels">${labelsHtml(issue.labels)}</div>` : ""}
    <button class="issue-assign" type="button" data-issue-assign="${esc(issue.id)}" aria-label="Assign #${esc(String(issue.number ?? ""))}" title="Assign this issue">
      ${assigneeHtml(issue.assignee, agentLabels)}
    </button>
  </li>`;

/** One column. The count is on the head because a board is read column by
 *  column, and "how much is in review" is the question a board is for. */
const columnHtml = (column, context) => `<section class="issue-column" data-column="${esc(column.id)}" aria-label="${esc(column.name)}">
    <header class="issue-column-head">
      <h3>${esc(column.name)}</h3>
      <span class="issue-column-count">${column.issues.length}</span>
    </header>
    <ul class="issue-column-cards" role="list" data-column-drop="${esc(column.id)}">
      ${column.issues.map((issue) => issueCardHtml(issue, context)).join("")}
    </ul>
  </section>`;

/**
 * The board.
 *
 * It scrolls sideways rather than reflowing: a column that wraps under another
 * is not a column any more, and on a phone the reader would lose the one thing
 * the layout is for. The sheet gives it that scroll under 760px and lets it sit
 * as a row above.
 */
export function boardHtml(board, context) {
  // A group rather than a list: its children are the COLUMNS, and a column is
  // a region with a heading and a list inside it, not a list item.
  return `<div class="issue-board" role="group" aria-label="Issues by column">
    <p class="sr-only">${esc(MOVE_HINT)}</p>
    ${board.map((column) => columnHtml(column, context)).join("")}
  </div>`;
}
