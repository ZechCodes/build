// A tracking notice, as one line in the conversation it lands in.
//
// Zech, with a screenshot of a project-agent conversation: "Tracking notices
// come in looking like user messages (same color and on the right). They
// should be a single line 'X did Y on Z' deep linking."
//
// They looked like that because that is what they are on the wire: a message
// on the user's side, marked `from_build`, carrying a `from_issue` envelope
// and a body that reads as prose. Drawn as an ordinary bubble it reads as
// though Zech wrote it — right-aligned, his colour, the whole comment body
// under it — which is three wrong claims in one row.
//
// So a message carrying both marks is a NOTICE: one quiet left-aligned line,
// the whole of it an anchor to the issue, and the comment body not shown at
// all. A press is what opens it; the line is the news that there is something
// to open.
//
// # Two sources, one shape
//
// `issue_notice` is the structured field (#35), and is read when it is there.
// Everything sent before it exists carries the same facts only as prose, so
// the body's first line is parsed as a fallback.
//
// The parse is allowed to fail. `from_issue` already carries the id, the
// number and the title, so the LINK never depends on it — only the actor and
// the verb do, and a line that cannot name them says the issue alone rather
// than nothing. A notice that degrades to "#32 Title" is still a notice, still
// on the left, still one line, and still opens the right page.
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { actionPhrase, actorName } from "./trackerLineWords.js";

export const NOTICE_CLASS = "thread-issue-notice";

/** Whether this message is a tracking notice rather than something a person
 *  or an agent said. Both marks, because `from_build` alone is the restart
 *  notice — which is an instruction to the agent and reads as one. */
export const isIssueNotice = (message) => Boolean(message?.from_build && message?.from_issue);

/**
 * The body's first line, as the facts it states.
 *
 * The shape the bridge writes is `#32 Some title — agent-01M… commented: …`.
 * Split at the LAST em dash before the tail, because a title may contain one
 * and the actor never does.
 */
function noticeFromBody(body) {
  const line = String(body || "").split("\n")[0].trim();
  const at = line.lastIndexOf(" — ");
  if (!line.startsWith("#") || at === -1) return null;
  const tail = line.slice(at + 3).trim();
  // `<actor> <verb>` up to the colon that introduces what was said.
  const said = tail.split(":")[0].trim();
  const gap = said.indexOf(" ");
  if (gap === -1) return null;
  return { actor: said.slice(0, gap), action: said.slice(gap + 1).trim() };
}

/**
 * One notice's facts, from the structured field where there is one and from
 * the body's prose where there is not.
 *
 * The issue itself always comes from the envelope: it is the one part that was
 * never prose, and the link is built from it whether the rest parses or not.
 */
/** Which issue, from the envelope first: it is the one part that was never
 *  prose, so it is the one part that cannot be lost to a parse. */
const issuePartOf = (envelope, stated) => ({
  issue_id: envelope.issue_id || stated?.issue_id || "",
  number: envelope.number ?? stated?.number ?? null,
  title: envelope.title || stated?.title || "",
});

/** Who did what, from the field where there is one and from the prose where
 *  there is not. The prose carries no comment id — there is nowhere in a
 *  sentence for one — so a fallback notice lands on the issue rather than on
 *  the comment, which is the right page either way. */
function actPartOf(stated, body) {
  if (stated) {
    return { actor: stated.actor || "", action: stated.action || "", comment_id: stated.comment_id || null };
  }
  const parsed = noticeFromBody(body) || {};
  return { actor: parsed.actor || "", action: parsed.action || "", comment_id: null };
}

export function issueNoticeOf(message) {
  if (!isIssueNotice(message)) return null;
  const stated = message.issue_notice || null;
  return { ...issuePartOf(message.from_issue || {}, stated), ...actPartOf(stated, message.body) };
}

/** Where the line goes: the issue's page, and the comment itself when the
 *  notice is about one. A conversation with no project to stand in writes no
 *  href, and the line draws as plain text rather than pointing nowhere. */
export function noticeHref(notice, place) {
  if (!place?.projectId || !notice?.issue_id) return "";
  const base = hashFromRoute({
    name: "trackerIssue",
    projectId: place.projectId,
    deviceId: place.deviceId ?? null,
    issueId: notice.issue_id,
  });
  return notice.comment_id ? `${base}#comment-${encodeURIComponent(notice.comment_id)}` : base;
}

/**
 * The line: "{actor} {action} on #{number} {title}".
 *
 * The actor and the verb are dropped when neither source named them, rather
 * than being guessed at or left as an empty gap — "#32 Title" is a true
 * sentence about an issue, and "acted on #32" is a claim nothing backs.
 */
/** "issues-spa · Agent 1 commented on", or nothing where neither source named
 *  both. Both halves or neither: "commented on #39" with nobody in front of it
 *  reads as the reader having done it. */
function openingHtml(notice, reading) {
  const who = actorName(notice.actor, reading);
  const did = actionPhrase(notice.action);
  return who && did ? `<span class="thread-issue-said">${esc(who)} ${esc(did)}</span>` : "";
}

const issueSaidHtml = (notice) =>
  `<span class="thread-issue-number">#${esc(String(notice.number ?? ""))}</span> <span class="thread-issue-line-title">${esc(notice.title || "")}</span>`;

export function issueNoticeLineHtml(notice, { place = null, agentLabels = {}, projectName = "" } = {}) {
  if (!notice?.issue_id) return "";
  // The spaces between the spans are for the reader, not for the layout: flex
  // drops whitespace-only nodes and `gap` does the spacing, but they stay in
  // the text a screen reader speaks and a copy takes.
  const opening = openingHtml(notice, { agentLabels, projectName });
  const said = `${opening ? `${opening} ` : ""}${issueSaidHtml(notice)}`;
  const href = noticeHref(notice, place);
  return href
    ? `<a class="${NOTICE_CLASS}" href="${esc(href)}" data-issue-notice="${esc(notice.issue_id)}">${said}</a>`
    : `<span class="${NOTICE_CLASS}" data-issue-notice="${esc(notice.issue_id)}">${said}</span>`;
}
