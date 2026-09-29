// The task card a message draws when it was the hand-off of a task.
//
// Assignment is dispatch: a task handed to an agent arrives in that agent's
// conversation as an ordinary message on the user's side, carrying a task
// envelope the way a hand-off carries `from_agent`. The body is the task
// rendered as prose, so a harness that never learns about `from_task` still
// receives the whole task; the envelope is for this client, which draws the
// card and links `#12`.
//
// It is still a message and still reads in sequence. The card is HOW it is
// drawn, not a separate kind of thing — which is why this renders above the
// body rather than replacing it.
//
// The task's own body is folded the way a long arrival is folded, by the same
// rule and at the same length: a hand-off carries the whole task, and the
// whole task is rarely what the next line of the conversation is about.
//
// The fold is not decided here. core/thread.js owns every fold a conversation
// has — the sent message's, the arrival's — so it measures this one too and
// hands over the press and whether the body is shut. There is one reading of
// "does this bury the conversation", and nothing here to drift from it.
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { markdownHtml } from "./markdown.js";
import { hashFromRoute } from "./router.js";
import { taskLinkRows } from "./trackerLinks.js";

const taskHref = (envelope, place) =>
  place?.projectId
    ? hashFromRoute({
        name: "trackerTask",
        projectId: place.projectId,
        deviceId: place.deviceId ?? null,
        taskId: envelope.task_id,
      })
    : "";

/**
 * The links the task carries, drawn under the card.
 *
 * Only the ones that READ without a lookup: a branch names itself and a parent
 * task names itself, while a workspace and a conversation are minted ids that
 * the feed has to name. A conversation is not a place to send a reader to with
 * `ws-3f2a91c4` written on the door — and the task page one press away has
 * the whole rail, each row named properly. So the card carries what a reader
 * can act on from inside a conversation, and `#12` carries the rest.
 */
function cardLinksHtml(envelope, place) {
  if (!place?.projectId) return "";
  const rows = taskLinkRows({ links: envelope.links }, place).filter(
    (row) => row.route && (row.kind === "branch" || row.kind === "parent"),
  );
  if (!rows.length) return "";
  return `<div class="thread-task-links">${rows
    .map((row) => `<a href="${esc(hashFromRoute(row.route))}">${esc(row.label)}</a>`)
    .join("")}</div>`;
}

/** The task's body, shut or open as the caller measured it, with the caller's
 *  press under it. The fold is a class on the body and the press names what it
 *  controls, which is the whole of how `wireThreadArrivals` reaches it. The
 *  body is markdown like every other body, through the one renderer (#229):
 *  it was escaped prose, so a task's headings, lists and references read as
 *  their marks on the card and as a document on the task's own page. */
const foldHtml = (body, { bodyId = "", folded = false, pressHtml = "", place = null }) =>
  // markdownHtml escapes all input before adding its fixed safe tag set.
  `<div class="thread-task-body${folded ? " thread-arrival-folded" : ""}"${bodyId ? ` id="${esc(bodyId)}"` : ""}><div class="thread-task-text markdown">${markdownHtml(body, { place })}</div></div>
    ${pressHtml || ""}`;

/**
 * The card.
 *
 * `envelope` is the message's `from_task`; `place` is where the reader is
 * standing, because every route under a project is written against the machine
 * that project is on.
 *
 * A message with no envelope draws nothing, which is every other message: the
 * card is opt-in on the record, so a client that has never heard of
 * `from_task` reads the conversation exactly as it always has.
 */
/** `#12`, as a link to the task's page — or as plain text for a conversation
 *  rendered with nowhere to send the reader. */
const numberHtml = (envelope, place) => {
  const number = `#${envelope.number ?? ""}`;
  const href = taskHref(envelope, place);
  return href
    ? `<a class="thread-task-link" href="${esc(href)}">${esc(number)}</a>`
    : `<span class="thread-task-link">${esc(number)}</span>`;
};

export function taskCardHtml(envelope, fold = {}) {
  if (!envelope || !envelope.task_id) return "";
  const place = fold.place || null;
  return `<div class="thread-task" data-task-card-for="${esc(envelope.task_id)}">
    <div class="thread-task-head">${numberHtml(envelope, place)}<span class="thread-task-title">${esc(envelope.title || "")}</span></div>
    ${envelope.body ? foldHtml(envelope.body, fold) : ""}
    ${cardLinksHtml(envelope, place)}
  </div>`;
}
