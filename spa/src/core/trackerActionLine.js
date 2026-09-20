// The line an agent leaves in its conversation when it acts on an issue.
//
// One line, in the agent's own voice — "commented #12 Kanban drag does not
// persist" — and the whole of it opens the issue. It is a message like any
// other: it reads in sequence, it counts as unread, and it is the agent
// saying what it just did rather than a notice from Build about it.
//
// Deliberately one line and no card. An agent that files, assigns, moves and
// comments across a working session would otherwise bury its own words under
// its own bookkeeping; the conversation is for what it said, and this is a
// footnote that happens to be clickable.
//
// A comment carries its own id, and the line lands on that comment rather than
// on the top of the issue — `#comment-<id>`, which the issue page's timeline
// rows answer to (core/trackerIssueRender.js).
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";

export const ACTION_LINE_CLASS = "thread-issue-action";

/**
 * How an action reads in a sentence.
 *
 * The bridge's own word is used as it comes; this only covers the four Zech
 * named, in case they arrive as bare tokens rather than as past tense. An
 * action this build has never heard of reads as itself rather than as nothing
 * — a later verb should leave a legible line, not a blank one.
 */
const ACTION_WORDS = Object.freeze({
  create: "created",
  created: "created",
  assign: "assigned",
  assigned: "assigned",
  update: "updated",
  updated: "updated",
  comment: "commented on",
  commented: "commented on",
  move: "moved",
  moved: "moved",
  close: "closed",
  closed: "closed",
  reopen: "reopened",
  reopened: "reopened",
  link: "linked",
  linked: "linked",
});

export const actionWord = (action) => {
  const said = String(action || "").trim();
  return ACTION_WORDS[said.toLowerCase()] || said || "acted on";
};

/**
 * Where the line goes: the issue's page, and the comment itself when the
 * action was a comment.
 *
 * A conversation rendered with nowhere to stand — no project — writes no href,
 * and the line draws as plain text rather than pointing nowhere.
 */
export function actionHref(action, place) {
  if (!place?.projectId || !action?.issue_id) return "";
  const base = hashFromRoute({
    name: "trackerIssue",
    projectId: place.projectId,
    deviceId: place.deviceId ?? null,
    issueId: action.issue_id,
  });
  return action.comment_id ? `${base}#comment-${encodeURIComponent(action.comment_id)}` : base;
}

/**
 * The line itself, or nothing for a message that carries no action.
 *
 * `#12` and the title are one anchor rather than two: the whole line is the
 * target, which is a larger press on a phone and one thing to tab to rather
 * than several.
 */
export function issueActionLineHtml(action, { place = null } = {}) {
  if (!action || !action.issue_id) return "";
  const said = `${actionWord(action.action)} <span class="thread-issue-action-number">#${esc(String(action.number ?? ""))}</span> <span class="thread-issue-action-title">${esc(action.title || "")}</span>`;
  const href = actionHref(action, place);
  return href
    ? `<a class="${ACTION_LINE_CLASS}" href="${esc(href)}" data-issue-action="${esc(action.issue_id)}">${said}</a>`
    : `<span class="${ACTION_LINE_CLASS}" data-issue-action="${esc(action.issue_id)}">${said}</span>`;
}
