// The issue card a message draws when it was the hand-off of an issue.
//
// Assignment is dispatch: an issue handed to an agent arrives in that agent's
// conversation as an ordinary message on the user's side, carrying an issue
// envelope the way a hand-off carries `from_agent`. The body is the issue
// rendered as prose, so a harness that never learns about `from_issue` still
// receives the whole issue; the envelope is for this client, which draws the
// card and links `#12`.
//
// It is still a message and still reads in sequence. The card is HOW it is
// drawn, not a separate kind of thing — which is why this renders above the
// body rather than replacing it.
//
// The issue's own body is folded the way a long arrival is folded, by the same
// rule and at the same length: a hand-off carries the whole issue, and the
// whole issue is rarely what the next line of the conversation is about.
//
// The fold is not decided here. core/thread.js owns every fold a conversation
// has — the sent message's, the arrival's — so it measures this one too and
// hands over the press and whether the body is shut. There is one reading of
// "does this bury the conversation", and nothing here to drift from it.
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { issueLinkRows } from "./trackerLinks.js";

const issueHref = (envelope, place) =>
  place?.projectId
    ? hashFromRoute({
        name: "trackerIssue",
        projectId: place.projectId,
        deviceId: place.deviceId ?? null,
        issueId: envelope.issue_id,
      })
    : "";

/**
 * The links the issue carries, drawn under the card.
 *
 * Only the ones that READ without a lookup: a branch names itself and a parent
 * issue names itself, while a workspace and a conversation are minted ids that
 * the feed has to name. A conversation is not a place to send a reader to with
 * `ws-3f2a91c4` written on the door — and the issue page one press away has
 * the whole rail, each row named properly. So the card carries what a reader
 * can act on from inside a conversation, and `#12` carries the rest.
 */
function cardLinksHtml(envelope, place) {
  if (!place?.projectId) return "";
  const rows = issueLinkRows({ links: envelope.links }, place).filter(
    (row) => row.route && (row.kind === "branch" || row.kind === "parent"),
  );
  if (!rows.length) return "";
  return `<div class="thread-issue-links">${rows
    .map((row) => `<a href="${esc(hashFromRoute(row.route))}">${esc(row.label)}</a>`)
    .join("")}</div>`;
}

/** The issue's body, shut or open as the caller measured it, with the caller's
 *  press under it. The fold is a class on the body and the press names what it
 *  controls, which is the whole of how `wireThreadArrivals` reaches it. */
const foldHtml = (body, { bodyId = "", folded = false, pressHtml = "" }) =>
  `<div class="thread-issue-body${folded ? " thread-arrival-folded" : ""}"${bodyId ? ` id="${esc(bodyId)}"` : ""}><span class="thread-issue-text">${esc(body)}</span></div>
    ${pressHtml || ""}`;

/**
 * The card.
 *
 * `envelope` is the message's `from_issue`; `place` is where the reader is
 * standing, because every route under a project is written against the machine
 * that project is on.
 *
 * A message with no envelope draws nothing, which is every other message: the
 * card is opt-in on the record, so a client that has never heard of
 * `from_issue` reads the conversation exactly as it always has.
 */
/** `#12`, as a link to the issue's page — or as plain text for a conversation
 *  rendered with nowhere to send the reader. */
const numberHtml = (envelope, place) => {
  const number = `#${envelope.number ?? ""}`;
  const href = issueHref(envelope, place);
  return href
    ? `<a class="thread-issue-link" href="${esc(href)}">${esc(number)}</a>`
    : `<span class="thread-issue-link">${esc(number)}</span>`;
};

export function issueCardHtml(envelope, fold = {}) {
  if (!envelope || !envelope.issue_id) return "";
  const place = fold.place || null;
  return `<div class="thread-issue" data-issue-card-for="${esc(envelope.issue_id)}">
    <div class="thread-issue-head">${numberHtml(envelope, place)}<span class="thread-issue-title">${esc(envelope.title || "")}</span></div>
    ${envelope.body ? foldHtml(envelope.body, fold) : ""}
    ${cardLinksHtml(envelope, place)}
  </div>`;
}
