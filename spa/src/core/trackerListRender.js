// The Issues tab's list view: the filter bar, and one row per issue.
//
// A row reads the way a GitHub issue row reads — the number, the title, the
// labels, then a second line of the facts that do not fit beside them — with
// two things GitHub does not have. Its status column is on the row, because
// this tracker has a board and "where is this" is half of what a row says. And
// the assignee is a PRESS, not a label: assigning is dispatching, so handing an
// issue to an agent is worth reaching from the list rather than only from the
// page.
//
// Pure: HTML in, no DOM, no app imports. core/trackerIssuesPane.js mounts it.

import { esc } from "./text.js";
import { filtersAreSet } from "./trackerFilters.js";
import { assigneeHtml, ageHtml, labelsHtml, numberHtml, priorityChipHtml, stateDotHtml, statusChipHtml } from "./trackerChips.js";

const optionHtml = (option, chosen) =>
  `<option value="${esc(option.value)}"${option.value === chosen ? " selected" : ""}>${esc(option.label)}</option>`;

const selectHtml = (name, label, options, chosen) => `<label class="issue-filter">
    <span class="sr-only">${esc(label)}</span>
    <select data-issue-filter="${esc(name)}" aria-label="${esc(label)}">${options.map((option) => optionHtml(option, chosen)).join("")}</select>
  </label>`;

/**
 * The filter bar.
 *
 * Every control here is a param of `issues.list`, one for one — a filter is a
 * question put to the bridge and not a pass over what happens to be in hand.
 * The Clear press is offered only once something is narrowed, so the bar does
 * not carry a dead control most of the time.
 */
export function filterBarHtml(options, filters) {
  return `<div class="issue-filters" role="group" aria-label="Filter issues">
    ${selectHtml("state", "State", options.states, filters.state)}
    ${selectHtml("status", "Column", options.statuses, filters.status)}
    ${selectHtml("assignee", "Assignee", options.assignees, filters.assignee)}
    ${selectHtml("label", "Label", options.labels, filters.label)}
    ${filtersAreSet(filters) ? `<button class="btn mini" type="button" data-issue-filter-clear>Clear</button>` : ""}
  </div>`;
}

/** Line two of a row: where it stands, what it weighs, and when it last moved.
 *  The parts that have nothing to say are left out rather than drawn empty, so
 *  the separator never leads. */
const factsHtml = (issue, columns, nowMs) =>
  [statusChipHtml(columns, issue.status), priorityChipHtml(issue.priority), ageHtml(issue.updated_at, nowMs)]
    .filter(Boolean)
    .join('<span class="issue-sep" aria-hidden="true">·</span>');

/**
 * One row.
 *
 * The whole row opens the issue; the assignee button inside it opens the
 * picker. Both are real controls — the row is a link so a middle-click and a
 * copied address work, and the button is a button so the keyboard reaches the
 * one press that starts an agent.
 */
export const issueRowHtml = (issue, { columns, agentLabels, href, nowMs = Date.now() }) =>
  `<li class="issue-row" data-issue="${esc(issue.id)}">
    <a class="issue-row-open" href="${esc(href(issue))}">
      ${stateDotHtml(issue.state)}
      <span class="issue-row-body">
        <span class="issue-row-head">${numberHtml(issue)}<span class="issue-title">${esc(issue.title)}</span>${labelsHtml(issue.labels)}</span>
        <span class="issue-row-facts">${factsHtml(issue, columns, nowMs)}</span>
      </span>
    </a>
    <button class="issue-assign" type="button" data-issue-assign="${esc(issue.id)}" aria-label="Assign #${esc(String(issue.number ?? ""))}" title="Assign this issue">
      ${assigneeHtml(issue.assignee, agentLabels)}
    </button>
  </li>`;

/** Nothing to show, said two ways: a project with no issues at all is at its
 *  first state and is told what to do about it, and a filter that matches
 *  nothing is told that IT is why the list is empty. */
export const emptyListHtml = (filters) =>
  filtersAreSet(filters)
    ? `<div class="empty issue-empty"><p>No issue matches these filters.</p></div>`
    : `<div class="empty issue-empty"><h2>No issues yet</h2><p>An issue is where you and the agents working this project agree on what is being done. New issue files the first one.</p></div>`;

export function issueListHtml(issues, context) {
  if (!issues.length) return emptyListHtml(context.filters || {});
  return `<ul class="issue-rows">${issues.map((issue) => issueRowHtml(issue, context)).join("")}</ul>`;
}
