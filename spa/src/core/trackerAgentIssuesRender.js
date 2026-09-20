// The issues entry in a conversation's activity area, as HTML.
//
// Four groups at most, in the order core/trackerAgentIssues.js puts them, with
// the last two folded. The fold is a `<details>`, which is what the surfaces
// beside this already use for their completed rows: the browser owns the
// toggle, so it works from the keyboard and from a screen reader without a line
// of wiring, and a repaint that leaves the element alone leaves it open.
//
// Nothing at all when the agent has no issues — not a heading over an empty
// box. An entry that is always present but usually empty is an entry a reader
// learns to skip.
//
// Pure: HTML in, no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { columnName } from "./trackerModel.js";
import { ageHtml, numberHtml, stateDotHtml } from "./trackerChips.js";

export const AGENT_ISSUES_CLASS = "agent-issues";
export const AGENT_ISSUE_CARD_CLASS = "agent-issue-card";
export const AGENT_ISSUE_FOLD_CLASS = "agent-issue-fold";

/** Where a card goes: the issue's own page, on the machine the project is on.
 *  A place that names no project writes no href, and the card draws unlinked
 *  rather than pointing nowhere. */
const cardHref = (issue, place) =>
  place?.projectId
    ? hashFromRoute({
        name: "trackerIssue",
        projectId: place.projectId,
        deviceId: place.deviceId ?? null,
        issueId: issue.id,
      })
    : "";

/**
 * One issue, as a card.
 *
 * The number and the title say which issue; the column and the time say where
 * it is and when it last moved — which together are the whole of what a reader
 * glancing at a conversation wants to know about it. Everything else is on the
 * page the card opens.
 */
export function agentIssueCardHtml(issue, { columns = null, place = null, nowMs = Date.now() } = {}) {
  const href = cardHref(issue, place);
  const body = `${stateDotHtml(issue.state)}${numberHtml(issue)}<span class="agent-issue-title">${esc(issue.title || "")}</span>
    <span class="agent-issue-facts"><span class="issue-status">${esc(columnName(columns, issue.status))}</span>${ageHtml(issue.updated_at, nowMs)}</span>`;
  return href
    ? `<a class="${AGENT_ISSUE_CARD_CLASS}" href="${esc(href)}" data-agent-issue="${esc(issue.id)}">${body}</a>`
    : `<div class="${AGENT_ISSUE_CARD_CLASS}" data-agent-issue="${esc(issue.id)}">${body}</div>`;
}

const cardsHtml = (issues, context) => issues.map((issue) => agentIssueCardHtml(issue, context)).join("");

/** A group that is news: a quiet heading and its cards, always open. */
const openGroupHtml = (group, context) => `<section class="agent-issue-group" data-agent-issue-group="${esc(group.id)}">
    <h4 class="agent-issue-group-head">${esc(group.label)}<span class="agent-issue-count">${group.issues.length}</span></h4>
    <div class="agent-issue-cards">${cardsHtml(group.issues, context)}</div>
  </section>`;

/** A group that is background: folded, with its count on the fold. The count is
 *  the point of a shut fold — it is the only thing it says — so it is never
 *  inside the part that hides. */
const foldedGroupHtml = (group, context) => `<details class="agent-issue-group ${AGENT_ISSUE_FOLD_CLASS}" data-agent-issue-group="${esc(group.id)}">
    <summary class="agent-issue-group-head">${esc(group.label)}<span class="agent-issue-count">${group.issues.length}</span></summary>
    <div class="agent-issue-cards">${cardsHtml(group.issues, context)}</div>
  </details>`;

/**
 * The whole entry, or nothing.
 *
 * `groups` is what `agentIssueGroups` answered — already ordered, already
 * missing its empty categories — so this only decides which of the two shapes
 * each one takes.
 */
export function agentIssuesHtml(groups, context = {}) {
  if (!groups?.length) return "";
  return `<div class="${AGENT_ISSUES_CLASS}" role="group" aria-label="Issues for this agent">
    ${groups.map((group) => (group.collapses ? foldedGroupHtml(group, context) : openGroupHtml(group, context))).join("")}
  </div>`;
}
