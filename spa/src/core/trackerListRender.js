// The Tasks tab's list view: one row per task.
//
// Two lines, and the split is the whole design (#28). Line one is `#12 Title`
// and nothing else, so the title has the row's full width and a column of
// titles reads as a column of titles. Everything else is on line two, in one
// order — where it stands, what it weighs, when it last moved, what it is
// tagged with, who holds it — separated by spacing and not by punctuation.
//
// Two things GitHub's row does not have. The status column is on the row,
// because this tracker has a board and "where is this" is half of what a row
// says. And the assignee is a PRESS, not a label: assigning is dispatching, so
// handing a task to an agent is worth reaching from the list rather than
// only from the page.
//
// That press is why the row's link covers line one rather than the whole row:
// a button cannot be nested inside an anchor. The anchor is stretched over the
// row in the stylesheet instead, so the row is still one thing to press and
// the button sits above it.
//
// Pure: HTML in, no DOM, no app imports. core/trackerTasksBody.js paints the
// rows this gives it, by key, into a list it keeps; the filter bar above them
// is mounted once and lives in core/trackerPaneChrome.js.

import { esc } from "./text.js";
import { actorHref } from "./trackerIdentity.js";
import { filtersAreSet } from "./trackerFilters.js";
import { taskBubbleHtml } from "./taskUnread.js";
import {
  ageHtml,
  assignPressLabel,
  closedChipHtml,
  numberHtml,
  priorityMarkHtml,
  rowAssigneeHtml,
  reviewerWords,
  rowLabelsHtml,
  statusChipHtml,
} from "./trackerChips.js";

/**
 * Line two, in the order the maintainer asked for it, quietly (#45).
 *
 * The same facts in the same order; what changed is how much each one weighs.
 * The column is the one chip and the only thing on the row with a background.
 * The age is dim. The labels are small muted words spaced apart rather than a
 * row of pills, three of them and a count for the rest. The holder is dim and
 * is still the press it was. The priority left this line entirely — it is a
 * mark before the title now, and only when it is pressing.
 *
 * Anything with nothing to say is left out rather than drawn empty — a stamp
 * that does not parse, a task with no labels — and what is left is spaced,
 * not punctuated: the gap between two of them is the separator.
 */
const factsHtml = (task, columns, nowMs, reading) =>
  [
    // Closed leads, because it changes how everything after it reads: a column
    // on a closed task is where it stopped, not where it is.
    closedChipHtml(task.state),
    statusChipHtml(columns, task.status, reviewerWords(task, reading)),
    ageHtml(task.updated_at, nowMs),
    rowLabelsHtml(task.labels),
  ]
    .filter(Boolean)
    .join("");

/**
 * One row.
 *
 * The whole row opens the task and the press on line two opens the picker.
 * Both are real controls — the row is a link so a middle-click and a copied
 * address work, and the press is a button so the keyboard reaches the one
 * action that starts an agent.
 *
 * No state dot. The row carries no open/closed mark at all now, which is a
 * thing the list used to say and no longer does; the board card, the agent's
 * entry and the task's own page all still carry it.
 *
 * Line one is `#12 Title` with, at most, one more thing on it: the mark a
 * pressing priority wears, between the two. It sits there rather than among
 * the facts because it says how to READ the title, and the eye going down a
 * column of titles meets it on the way in (#45). A watched task with unread
 * wears its bubble at the line's end, where the inbox's rows wear theirs
 * (#104). `unreadOf` is the pane's count, which reads the cached timeline.
 */
export const taskRowHtml = (task, { columns, href, nowMs = Date.now(), unreadOf, ...context }) => {
  const reading = { ...context, identities: task.identities || {} };
  const assigneeHref = task.assignee && actorHref(task.assignee, reading);
  const assignButton = `<button class="task-assign" type="button" data-task-assign="${esc(task.id)}" aria-label="${esc(assignPressLabel(task, reading))}" title="Assign this task">${assigneeHref ? "Change" : rowAssigneeHtml(task.assignee, reading)}</button>`;
  const assignee = assigneeHref
    ? `<span class="task-assignee-entry"><a class="task-assignee-link" href="${esc(assigneeHref)}">${rowAssigneeHtml(task.assignee, reading)}</a>${assignButton}</span>`
    : assignButton;
  return (
  `<li class="task-row" data-task="${esc(task.id)}">
    <a class="task-row-open" href="${esc(href(task))}">${numberHtml(task)}${priorityMarkHtml(task.priority)}<span class="task-title">${esc(task.title)}</span>${taskBubbleHtml(task, unreadOf)}</a>
    <span class="task-row-facts">${factsHtml(task, columns, nowMs, reading)}${assignee}</span>
  </li>`);
};

/** Nothing to show, said two ways: a project with no tasks at all is at its
 *  first state and is told what to do about it, and a filter that matches
 *  nothing is told that IT is why the list is empty. */
export const emptyListHtml = (filters) =>
  filtersAreSet(filters)
    ? `<div class="empty task-empty"><p>No task matches these filters.</p></div>`
    : `<div class="empty task-empty"><h2>No tasks yet</h2><p>A task is where you and the agents working this project agree on what is being done. New task files the first one.</p></div>`;
