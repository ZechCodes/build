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
import { actionPhrase } from "./trackerLineWords.js";

export const ACTION_LINE_CLASS = "thread-issue-action";

/** The verb. Mid-sentence now: the number leads the line (#49), so what
 *  happened is no longer opening it. */
export const actionWord = (action) => actionPhrase(action);

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
 * The words of the line. A creation is the one action whose news is the title
 * — nothing else on the page has named the issue yet — so it reads verb first
 * with the title, ellipsised: Zech, 21:19Z: "Use 'Created #X {title}'".
 * Every other action leads with the number and leaves the title to hover.
 */
function actionSpansHtml(action) {
  const number = `<span class="thread-issue-number">#${esc(String(action.number ?? ""))}</span>`;
  const word = actionWord(action.action);
  if (word === "created") {
    const title = action.title ? ` <span class="thread-issue-title">${esc(action.title)}</span>` : "";
    return `<span class="thread-issue-said">Created</span> ${number}${title}`;
  }
  return `${number} <span class="thread-issue-said">${esc(word)}</span>`;
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
  // The number first, then what was done (#49). No actor: this line IS the
  // agent speaking in its own conversation, so "by …" would name the voice
  // already saying it. No title either — it was the longest part of the line
  // and the first to be cut off, and it is the heading of the page the link
  // opens. Hover carries it, where length costs nothing.
  //
  // The spaces between the spans are for the reader, not for the layout: flex
  // drops whitespace-only nodes and `gap` does the spacing, but they stay in
  // the text a screen reader speaks and a copy takes.
  const said = actionSpansHtml(action);
  const hover = action.title ? ` title="${esc(action.title)}"` : "";
  const href = actionHref(action, place);
  return href
    ? `<a class="${ACTION_LINE_CLASS}" href="${esc(href)}"${hover} data-issue-action="${esc(action.issue_id)}">${said}</a>`
    : `<span class="${ACTION_LINE_CLASS}"${hover} data-issue-action="${esc(action.issue_id)}">${said}</span>`;
}
