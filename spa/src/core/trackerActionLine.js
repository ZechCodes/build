// The line an agent leaves in its conversation when it acts on a task.
//
// One line, in the agent's own voice — "Commented on #12" — and the whole of
// it opens the task. It is a message like any
// other: it reads in sequence, it counts as unread, and it is the agent
// saying what it just did rather than a notice from Build about it.
//
// Deliberately one line and no card. An agent that files, assigns, moves and
// comments across a working session would otherwise bury its own words under
// its own bookkeeping; the conversation is for what it said, and this is a
// footnote that happens to be clickable.
//
// A comment carries its own id, and the line lands on that comment rather than
// on the top of the task — `#comment-<id>`, which the task page's timeline
// rows answer to (core/trackerTaskRender.js).
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { actionPhrase, actorName, columnName, quoted, shownName } from "./trackerLineWords.js";

export const ACTION_LINE_CLASS = "thread-task-action";

/** The verb, as the bridge's token reads; the line capitalises it, since it
 *  opens the line (#323). */
export const actionWord = (action) => actionPhrase(action);

/**
 * Where the line goes: the task's page, and the comment itself when the
 * action was a comment.
 *
 * A conversation rendered with nowhere to stand — no project — writes no href,
 * and the line draws as plain text rather than pointing nowhere.
 */
export function actionHref(action, place) {
  if (!place?.projectId || !action?.task_id) return "";
  return hashFromRoute({
    name: "trackerTask",
    projectId: place.projectId,
    deviceId: place.deviceId ?? null,
    taskId: action.task_id,
    ...(action.comment_id ? { commentId: action.comment_id } : null),
  });
}

/**
 * The words of the line, verb first in the agent's own voice (#323, the
 * maintainer: "Commented on #111", "Moved #111 to “In Review”").
 *
 * Three actions say more than the verb. A creation keeps the title, because
 * its news is the title — nothing else on the page has named the task yet:
 * "Created #X {title}". An assignment names WHO got it: "Assigned #52 to
 * Agent 1". A move names the column it went to, quoted.
 *
 * Whatever the record does not carry — an assignee or a column from an older
 * bridge — is left out rather than invented: "Assigned #52", "Moved #52".
 */
const part = (kind, text, hover = "") => ({ kind, text, hover });

const ACTION_DETAILS = Object.freeze({
  created: (action) => (action.title ? [part("title", action.title)] : []),
  assigned: (action, reading) => {
    const who = actorName(action.assignee, reading);
    return who ? [part("said", "to"), part("who", shownName(who), who)] : [];
  },
  moved: (action) => (action.to ? [part("said", `to ${quoted(columnName(action.to))}`)] : []),
});

const capitalised = (text) => text.charAt(0).toUpperCase() + text.slice(1);

/** One set of words supplies both the visible line and its full hover text. */
function actionParts(action, reading) {
  const word = actionWord(action.action);
  return [part("said", capitalised(word)), part("number", `#${action.number ?? ""}`),
    ...(ACTION_DETAILS[word]?.(action, reading) ?? [])];
}

const partHtml = ({ kind, text, hover }) =>
  `<span class="thread-task-${kind}"${hover ? ` title="${esc(hover)}"` : ""}>${esc(text)}</span>`;

/**
 * The line itself, or nothing for a message that carries no action.
 *
 * `#12` and the title are one anchor rather than two: the whole line is the
 * target, which is a larger press on a phone and one thing to tab to rather
 * than several.
 */
export function taskActionLineHtml(action, { place = null, agentLabels = {}, projectName = "" } = {}) {
  if (!action || !action.task_id) return "";
  // What was done, then the number (#323). No actor: this line IS the
  // agent speaking in its own conversation, so "by …" would name the voice
  // already saying it. No title either — it was the longest part of the line
  // and the first to be cut off, and it is the heading of the page the link
  // opens. Hover carries the full line and title, where length costs nothing.
  const parts = actionParts(action, { agentLabels, projectName });
  const said = parts.map(partHtml).join(" ");
  const lineText = parts.map(({ text }) => text).filter(Boolean).join(" ");
  // A creation already names the task title in its line.
  const title = actionWord(action.action) === "created" ? "" : action.title;
  const hover = ` title="${esc([lineText, title].filter(Boolean).join(" — "))}"`;
  const href = actionHref(action, place);
  return href
    ? `<a class="${ACTION_LINE_CLASS}" href="${esc(href)}"${hover} data-task-action="${esc(action.task_id)}">${said}</a>`
    : `<span class="${ACTION_LINE_CLASS}"${hover} data-task-action="${esc(action.task_id)}">${said}</span>`;
}
