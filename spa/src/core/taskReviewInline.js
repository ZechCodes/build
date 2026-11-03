// Saved task threads and their reply actions share one presentation.
import "../styles/taskReviewInline.css";
import { esc } from "./text.js";
import { markdownHtml } from "./markdown.js";
import { actorIdentityHtml } from "./trackerIdentity.js";
import { ageText } from "./trackerChips.js";
import { advisoryReviewHtml } from "./taskReviewOpinion.js";

const commentsOf = (comments) => (comments || []).map((row) => row.comment || row)
  .filter((row) => !row.type || row.type === "comment");
const idOf = (row) => row.key || row.id;
const replyOf = (row) => row.replyTo || row.reply_to;
const matchesCommit = (commit, context) => context.allowedCommits
  ? context.allowedCommits.includes(commit || "") : (commit || "") === (context.commit || "");

function matchesAnchor(anchor, context) {
  if (!anchor) return false;
  return anchor.snapshot_id === context.snapshotId && anchor.directory_id === context.directoryId
    && anchor.path === context.path && anchor.side === context.side
    && Number(anchor.line) === Number(context.line)
    && matchesCommit(anchor.commit, context);
}

function threadComments(comments, context) {
  const rows = commentsOf(comments);
  const matching = new Set(rows.filter((row) => matchesAnchor(row.anchor, context)).map(idOf));
  // Replies retain their parent's anchor. Iterate to include nested replies in
  // timeline order without assigning them a new line or revision.
  let previous = -1;
  while (previous !== matching.size) {
    previous = matching.size;
    rows.forEach((row) => { if (matching.has(replyOf(row))) matching.add(idOf(row)); });
  }
  return rows.filter((row) => matching.has(idOf(row)));
}

function commentHtml(row, context) {
  const at = row.at || row.created_at;
  const time = at ? `<time title="${esc(at)}">${esc(ageText(at))}</time>` : "";
  const body = markdownHtml(row.body || "", {
    place: { deviceId: context.deviceId, projectId: context.projectId }, identities: context.identities,
  });
  // markdownHtml and actorIdentityHtml return escaped markup; all scalar attributes use esc.
  // nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format
  return `<article class="task-review-inline-comment" data-review-inline-comment="${esc(idOf(row) || "")}"><header><strong>${actorIdentityHtml(row.actor || row.author, context)}</strong>${time}${advisoryReviewHtml(row, context)}${context.replyable ? `<button class="btn" type="button" data-review-inline-reply="${esc(idOf(row) || "")}">Reply</button>` : ""}</header><div class="markdown">${body}</div></article>`;
}

/** HTML immediately after one source/diff row. Scope includes the comparison's
 * commit: a range comment never silently becomes a per-commit comment. */
export function inlineReviewRowsHtml(comments, context) {
  const rows = threadComments(comments, context);
  if (!rows.length) return "";
  const colspan = context.colspan || 3;
  return `<tr class="task-review-inline" data-review-inline><td colspan="${colspan}"><div class="task-review-inline-thread" aria-label="Comments on ${esc(context.side)} line ${Number(context.line)}">${rows.map((row) => commentHtml(row, context)).join("")}</div></td></tr>`;
}

/** Files pages append rows as they arrive; insert each thread after its line
 * without rebuilding or exposing the source bytes already in the viewer. */
export function paintInlineReviewRows(host, comments, context) {
  host.querySelectorAll("[data-review-inline]").forEach((row) => row.remove());
  host.querySelectorAll("tr[data-new-line]").forEach((row) => {
    // The shared renderer escapes text and uses the application markdown safe tag set.
    // nosemgrep: typescript.react.security.audit.react-unsanitized-method.react-unsanitized-method
    row.insertAdjacentHTML("afterend", inlineReviewRowsHtml(comments, {
      ...context, side: "new", line: Number(row.dataset.newLine), colspan: context.colspan || 2,
    }));
  });
}

export function reviewComposerMatches(anchor, context) {
  if (!anchor || anchor.snapshot_id !== context.snapshotId || anchor.directory_id !== context.directoryId) return false;
  if (anchor.side === "old" && context.comparisonMode === "since" && context.sinceBaseline !== "base") return false;
  return matchesCommit(anchor.commit, context);
}

/** Move the same editor node after a line, preserving focus and typed text. */
export function placeReviewComposer(host, composer, anchor) {
  if (!anchor) return false;
  const sections = [...host.querySelectorAll('[data-review-path]')];
  const section = sections.length ? sections.find((row) => row.dataset.reviewPath === anchor.path) : host;
  if (!section) return false;
  const key = anchor.side === "old" ? "oldLine" : "newLine";
  const line = [...section.querySelectorAll('tr[data-new-line], tr[data-old-line]')]
    .find((row) => Number(row.dataset[key]) === Number(anchor.line));
  if (!line) return false;
  composer.querySelector('td').colSpan = line.children.length;
  if (line.nextElementSibling !== composer) line.after(composer);
  return true;
}

export function reviewReplyAt(event, comments, onReply, anchorOf) {
  const button = event.target.closest('[data-review-inline-reply]');
  if (!button || !onReply) return false;
  const id = button.dataset.reviewInlineReply;
  const row = commentsOf(comments).find((row) => idOf(row) === id);
  if (!row) return false;
  const line = button.closest('[data-review-inline]')?.previousElementSibling;
  onReply(row, inheritedAnchor(row, comments) || anchorOf(line));
  return true;
}

function inheritedAnchor(row, comments) {
  let parent = row;
  const seen = new Set();
  while (!parent.anchor && replyOf(parent) && !seen.has(idOf(parent))) {
    seen.add(idOf(parent));
    const previous = commentsOf(comments).find((entry) => idOf(entry) === replyOf(parent));
    if (!previous) break;
    parent = previous;
  }
  return parent.anchor;
}
