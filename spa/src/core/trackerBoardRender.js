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
import { actorHref } from "./trackerIdentity.js";
import { KEYED_LIST_ATTRIBUTE } from "./domPatch.js";
import {
  assigneeHtml, labelsHtml, numberHtml, priorityChipHtml, reviewerWords, stateDotHtml, statusChipHtml,
} from "./trackerChips.js";
import { columnNote } from "./trackerModel.js";

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
export const issueCardHtml = (issue, { columns, href, ...context }) => {
  const reading = { ...context, identities: issue.identities || {} };
  const assigneeHref = issue.assignee && actorHref(issue.assignee, reading);
  const assignButton = `<button class="issue-assign" type="button" data-issue-assign="${esc(issue.id)}" aria-label="Assign #${esc(String(issue.number ?? ""))}" title="Assign this issue">${assigneeHref ? "Change" : assigneeHtml(issue.assignee, reading)}</button>`;
  // The column already says In review; the card says who it is with (#144).
  const withWhom = reviewerWords(issue, reading);
  return (
  `<li class="issue-card" draggable="true" tabindex="0"
      data-issue="${esc(issue.id)}" data-status="${esc(issue.status)}"
      aria-label="#${esc(String(issue.number ?? ""))} ${esc(issue.title)}">
    <div class="issue-card-head">${stateDotHtml(issue.state)}${numberHtml(issue)}${priorityChipHtml(issue.priority)}</div>
    <a class="issue-card-title" href="${esc(href(issue))}">${esc(issue.title)}</a>
    ${withWhom ? `<div class="issue-card-status">${statusChipHtml(columns, issue.status, withWhom)}</div>` : ""}
    ${issue.labels?.length ? `<div class="issue-card-labels">${labelsHtml(issue.labels)}</div>` : ""}
    ${assigneeHref
      ? `<span class="issue-assignee-entry"><a class="issue-assignee-link" href="${esc(assigneeHref)}">${assigneeHtml(issue.assignee, reading)}</a>${assignButton}</span>`
      : assignButton}
  </li>`);
};

/**
 * What a column means, said twice over.
 *
 * A `title` for a pointer, and a `details` behind an info glyph for everything
 * that has no pointer — a phone, a keyboard, a screen reader. The same words
 * both ways, from the Issues Spec (core/trackerModel.js): this board is the
 * one place the user meets rules the agents are told outright and the user
 * never is, and "In review means ready to look at, not accepted" is the whole
 * reason a first-time reader misreads it.
 *
 * `details` rather than a tooltip of our own, as the folds beside it already
 * are: the browser owns the press, so it answers a tap, Enter and a screen
 * reader without a line of wiring.
 */
const columnWhyHtml = (column, note) => `<details class="issue-column-why">
      <summary aria-label="What ${esc(column.name)} means" title="${esc(note)}">i</summary>
      <p class="issue-column-note" role="note">${esc(note)}</p>
    </details>`;

/**
 * One column, with no cards in it.
 *
 * The count is on the head because a board is read column by column, and "how
 * much is in review" is the question a board is for. The cards themselves are
 * painted into the list by key (core/trackerIssuesBody.js), which is what
 * `data-keyed-list` says: this frame is patched on every paint, and the patch
 * stops at the list rather than rebuilding cards that did not change.
 */
const columnHtml = (column, context) => {
  const note = columnNote(context.columns, column.id);
  return `<section class="issue-column" data-column="${esc(column.id)}" aria-label="${esc(column.name)}">
    <header class="issue-column-head" title="${esc(note)}">
      <h3>${esc(column.name)}</h3>
      ${columnWhyHtml(column, note)}
      <span class="issue-column-count">${column.issues.length}</span>
    </header>
    <ul class="issue-column-cards" role="list" data-column-drop="${esc(column.id)}" ${KEYED_LIST_ATTRIBUTE}></ul>
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
  return `<div class="issue-board" role="group" aria-label="Issues by column">
    <p class="sr-only">${esc(MOVE_HINT)}</p>
    ${board.map((column) => columnHtml(column, context)).join("")}
  </div>`;
}
