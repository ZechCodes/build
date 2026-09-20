// The Issues tab's list view: one row per issue.
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
// handing an issue to an agent is worth reaching from the list rather than
// only from the page.
//
// That press is why the row's link covers line one rather than the whole row:
// a button cannot be nested inside an anchor. The anchor is stretched over the
// row in the stylesheet instead, so the row is still one thing to press and
// the button sits above it.
//
// Pure: HTML in, no DOM, no app imports. core/trackerIssuesBody.js paints the
// rows this gives it, by key, into a list it keeps; the filter bar above them
// is mounted once and lives in core/trackerPaneChrome.js.

import { esc } from "./text.js";
import { filtersAreSet } from "./trackerFilters.js";
import {
  ageHtml,
  assignPressLabel,
  closedChipHtml,
  labelsHtml,
  numberHtml,
  priorityChipHtml,
  rowAssigneeHtml,
  statusChipHtml,
} from "./trackerChips.js";

/** Line two, in the order Zech asked for it. Anything with nothing to say is
 *  left out rather than drawn empty — a priority of `none`, a stamp that does
 *  not parse, an issue with no labels — and what is left is spaced, not
 *  punctuated: the gap between two chips is the separator. */
const factsHtml = (issue, columns, nowMs) =>
  [
    // Closed leads, because it changes how everything after it reads: a column
    // and a priority on a closed issue are where it stopped, not where it is.
    closedChipHtml(issue.state),
    statusChipHtml(columns, issue.status),
    priorityChipHtml(issue.priority),
    ageHtml(issue.updated_at, nowMs),
    labelsHtml(issue.labels),
  ]
    .filter(Boolean)
    .join("");

/**
 * One row.
 *
 * The whole row opens the issue and the press on line two opens the picker.
 * Both are real controls — the row is a link so a middle-click and a copied
 * address work, and the press is a button so the keyboard reaches the one
 * action that starts an agent.
 *
 * No state dot. The row carries no open/closed mark at all now, which is a
 * thing the list used to say and no longer does; the board card, the agent's
 * entry and the issue's own page all still carry it.
 */
export const issueRowHtml = (issue, { columns, agentLabels, href, nowMs = Date.now() }) =>
  `<li class="issue-row" data-issue="${esc(issue.id)}">
    <a class="issue-row-open" href="${esc(href(issue))}">${numberHtml(issue)}<span class="issue-title">${esc(issue.title)}</span></a>
    <span class="issue-row-facts">${factsHtml(issue, columns, nowMs)}<button class="issue-assign" type="button" data-issue-assign="${esc(issue.id)}" aria-label="${esc(assignPressLabel(issue, agentLabels))}" title="Assign this issue">${rowAssigneeHtml(issue.assignee, agentLabels)}</button></span>
  </li>`;

/** Nothing to show, said two ways: a project with no issues at all is at its
 *  first state and is told what to do about it, and a filter that matches
 *  nothing is told that IT is why the list is empty. */
export const emptyListHtml = (filters) =>
  filtersAreSet(filters)
    ? `<div class="empty issue-empty"><p>No issue matches these filters.</p></div>`
    : `<div class="empty issue-empty"><h2>No issues yet</h2><p>An issue is where you and the agents working this project agree on what is being done. New issue files the first one.</p></div>`;
