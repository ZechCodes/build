// The issue page, as HTML: the issue, its timeline, the composer, and the rail
// of everything about it that can be changed.
//
// The rail's ordering is the ordering of how often a thing is touched, not of
// how important it sounds: state, then the column, then the labels and the
// priority, then who holds it, then what it is linked to. Assigning sits low
// because it is the one press that starts an agent, and a control that starts
// work should not be the first thing under a thumb.
//
// Pure: HTML in, no DOM, no app imports. core/trackerIssuePage.js mounts it.

import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";
import { hashFromRoute } from "./router.js";
import { columnsOf, PRIORITIES, stateLabel } from "./trackerModel.js";
import { issueAvatarHtml } from "./issueAvatar.js";
import { actorIdentityHtml, actorHref } from "./trackerIdentity.js";
import { watchButtonHtml } from "./watchToggle.js";
import { eventSentence } from "./trackerTimeline.js";
import { issueUnreadKey } from "./trackerUnread.js";
import { ageHtml, ageText, assigneeHtml, labelsHtml, numberHtml, stateDotHtml } from "./trackerChips.js";
import { attachmentGlyphHtml, composerPartIds, formatAttachmentSize, isImageAttachment } from "./composer.js";
import { ICON_PAPERCLIP } from "./icons.js";

/** The head: what the issue is called, and the two facts that are independent
 *  of each other — is it still open, and where does it stand on the board.
 *
 *  `watch` is the switch's state when this device's bridge can be asked about
 *  watching (#65) and nothing at all when it cannot: a control that refuses
 *  every press is worse than one the reader has not been given yet. */
export const issueHeadHtml = (issue, { watch = null } = {}) => `<header class="issue-page-head">
    <div class="issue-page-marks">
      ${stateDotHtml(issue.state)}<span class="issue-page-state">${esc(stateLabel(issue.state))}</span>
      ${numberHtml(issue)}
      ${ageHtml(issue.updated_at)}
      ${watch ? watchButtonHtml(watch) : ""}
    </div>
    <h1 class="issue-page-title">${esc(issue.title)}</h1>
    ${issue.labels?.length ? `<div class="issue-page-labels">${labelsHtml(issue.labels)}</div>` : ""}
  </header>`;

/** The body, as markdown. An issue with an empty body says so rather than
 *  leaving a gap a reader has to interpret. */
export const issueBodyHtml = (issue, refLinks = null) =>
  issue.body
    // renderMarkdown escapes all input before adding its fixed safe tag set.
    ? `<div class="issue-page-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(issue.body, { links: refLinks })}</div>`
    : `<p class="issue-page-body empty">No description.</p>`;

/**
 * The files filed with the issue (#57).
 *
 * Deliberately the conversation's own markup and classes, down to
 * `data-attachment-path`: one attachment story in this client rather than two.
 * That is what lets `wireThreadAttachments` fill these — the loading, the
 * refusal and the wire-went-away deferral are all already written, and a
 * second copy of them would be a second set of answers about one failure.
 *
 * `src` is left empty and filled by that wiring, because the page is a string
 * and the bytes are a round trip away.
 */
export const issueAttachmentsHtml = (attachments) => {
  const held = (attachments || []).filter((one) => one && one.path);
  if (!held.length) return "";
  return `<div class="thread-attachments issue-page-attachments">${held.map(attachmentHtml).join("")}</div>`;
};

const attachmentHtml = (attachment) => {
  const path = esc(attachment.path || "");
  const name = esc(attachment.name || attachment.path || "file");
  const size = esc(formatAttachmentSize(attachment.size));
  if (isImageAttachment(attachment.mime)) {
    return `<figure class="thread-attachment-figure">
      <button type="button" class="thread-attachment-preview" aria-label="Open ${name}">
        <img class="thread-attachment-image" data-attachment-path="${path}" alt="${name}">
      </button>
      <figcaption><span class="thread-attachment-name">${name}</span> <span class="thread-attachment-size">${size}</span></figcaption>
    </figure>`;
  }
  return `<button type="button" class="thread-attachment" data-attachment-path="${path}" data-attachment-name="${name}" title="Download ${name}">
    ${attachmentGlyphHtml(attachment.name, attachment.mime, "thread-attachment-glyph")}
    <span class="thread-attachment-meta">
      <span class="thread-attachment-name">${name}</span>
      <span class="thread-attachment-size">${size}</span>
    </span>
  </button>`;
};

const whenHtml = (row) => (row.at ? `<span class="issue-when" title="${esc(row.at)}">${esc(ageText(row.at))}</span>` : "");

/// The id an action line in a conversation lands on: a comment is linked as
/// `#comment-<id>` (core/trackerActionLine.js), so the row has to answer to it.
const commentHtml = (row, context) => `<li class="issue-entry issue-comment${row.mentionsUser ? " issue-comment-mentioned" : ""}" id="comment-${esc(row.key)}">
    ${issueAvatarHtml(row.actor, context)}
    <div class="issue-comment-card">
      <div class="issue-entry-head"><strong>${actorIdentityHtml(row.actor, context)}</strong>${whenHtml(row)}</div>
      <div class="issue-comment-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(row.body, { links: context.refLinks })}</div>
    </div>
  </li>`;

/** An event is one line: who, what they did, and when. It is history, so it is
 *  drawn quieter than a comment — but never hidden, because "the board moved
 *  and nobody said anything" is exactly what a timeline is for. */
const assignedDetailHtml = (row, context) =>
  `assigned this to ${row.payload?.assignee ? actorIdentityHtml(row.payload.assignee, context, { icon: true }) : "nobody"}`;

const workspaceMentionHtml = (workspaceId, context) => {
  const workspace = (context.links || []).find((link) => link.kind === "workspace" && link.workspaceId === workspaceId);
  const label = esc(workspace?.label || workspaceId);
  return workspace?.route ? `<a href="${esc(hashFromRoute(workspace.route))}">${label}</a>` : label;
};

const dispatchedDetailHtml = (row, context) => {
  const agent = actorIdentityHtml({ kind: "agent", agent_id: row.payload.agent_id }, context, { icon: true });
  const where = row.payload.workspace_id ? ` in ${workspaceMentionHtml(row.payload.workspace_id, context)}` : "";
  return `started ${agent}${where} on this`;
};

const linkedWorkspaceHtml = (row, context) =>
  `linked workspace ${workspaceMentionHtml(row.payload.workspace_id, context)}`;

const eventDetailHtml = (row, context) => {
  if (row.kind === "assigned") return assignedDetailHtml(row, context);
  if (row.kind === "dispatched" && row.payload?.agent_id) return dispatchedDetailHtml(row, context);
  if (row.kind === "linked" && row.payload?.workspace_id) return linkedWorkspaceHtml(row, context);
  return esc(eventSentence(row, context));
};

const eventHtml = (row, context) => `<li class="issue-entry issue-event">
    <span class="issue-event-dot" aria-hidden="true"></span>
    <span class="issue-event-text"><strong>${actorIdentityHtml(row.actor, context, { icon: true })}</strong> ${eventDetailHtml(row, context)}</span>
    ${whenHtml(row)}
  </li>`;

/** The timeline: comments and events interleaved, ascending, in the order the
 *  bridge answered them. Never re-sorted here — see core/trackerTimeline.js. */
export function timelineHtml(rows, context) {
  if (!rows.length) return `<p class="empty issue-empty">Nothing has happened on this issue yet.</p>`;
  return `<ul class="issue-timeline">${rows
    .map((row) => {
      const line = context.unreadFrom != null && context.unreadFrom === issueUnreadKey(row.key)
        ? '<li class="thread-unread-line issue-unread-line" role="separator"><span>New</span></li>' : "";
      return line + (row.type === "comment" ? commentHtml(row, context) : eventHtml(row, context));
    })
    .join("")}</ul>`;
}

/// The ids `mountComposerAttachments` reads on the comment box. The textarea
/// keeps the id it always had, so everything already addressing it still does.
export const COMMENT_INPUT_ID = "issue-comment";
const commentParts = composerPartIds(COMMENT_INPUT_ID);

/**
 * The comment box.
 *
 * `attachable` is whether this device's bridge can carry files on an issue
 * (core/issueAttachments.js). One that cannot gets the plain box it always had
 * — no paperclip, no tray, no drop mask — because an affordance that is drawn
 * and then apologised for is worse than one that was never offered.
 *
 * The send press is enabled by a draft OR by a tray with something in it: a
 * comment that is only a screenshot is a comment.
 */
/// The paperclip, the hidden picker and the drop mask — or nothing at all.
const commentAttachHtml = () => `<div class="composer-bar">
      <div class="composer-actions">
        <input type="file" id="${commentParts.file}" class="composer-file" multiple hidden>
        <button type="button" class="composer-attach" id="${commentParts.attach}" aria-label="Attach files" title="Attach files">${ICON_PAPERCLIP}</button>
      </div>
    </div>
    <div class="composer-dropmask" aria-hidden="true"><span>Drop to attach</span></div>`;

/// Whether the send press can be pressed. A comment that is only a screenshot
/// is a comment, so a tray with something in it is as good as a draft.
const canComment = (draft, busy, hasFiles) => !busy && (Boolean(draft.trim()) || hasFiles);

const commentFieldHtml = (draft, busy) =>
  `<textarea id="${COMMENT_INPUT_ID}" rows="3" placeholder="Comment on this issue"${busy ? " disabled" : ""}>${esc(draft)}</textarea>`;

/// The box, wrapped or bare. A bridge that cannot carry files gets exactly the
/// box it always had — the same element, unwrapped — so gating the paperclip
/// costs an older bridge nothing at all, not even a changed frame.
const commentBoxHtml = (draft, busy, attachable) =>
  attachable
    ? `<div class="composer-tray" id="${commentParts.tray}" hidden></div>
    <div class="composer attachable issue-comment-box">
      ${commentFieldHtml(draft, busy)}
      ${commentAttachHtml()}
    </div>`
    : commentFieldHtml(draft, busy);

export const composerHtml = (draft, busy, attachable = false, hasFiles = false) => `<form class="issue-composer" data-issue-composer>
    <label class="sr-only" for="${COMMENT_INPUT_ID}">Comment on this issue</label>
    ${commentBoxHtml(draft, busy, attachable)}
    <div class="row issue-composer-row">
      <button class="btn primary" type="submit"${canComment(draft, busy, hasFiles) ? "" : " disabled"}>${busy ? "sending…" : "Comment"}</button>
    </div>
  </form>`;

// ---- the rail ---------------------------------------------------------------

const railSection = (title, inner) => `<section class="issue-rail-section"><h2>${esc(title)}</h2>${inner}</section>`;

/** Close and reopen, and nothing else on this control. Closing an issue stops
 *  nothing and starts nothing — it says the work is over — so the button says
 *  only that. */
const stateControlHtml = (issue, busy) => railSection(
  "State",
  `<button class="btn" type="button" data-issue-state${busy ? " disabled" : ""}>${issue.state === "closed" ? "Reopen issue" : "Close issue"}</button>
   <p class="sub">${issue.state === "closed" ? "Closed issues keep their column." : "Closing does not move it to Done."}</p>`,
);

const selectRow = (id, label, optionsHtml, busy) => `<label class="create-label" for="${esc(id)}">${esc(label)}</label>
  <select id="${esc(id)}"${busy ? " disabled" : ""}>${optionsHtml}</select>`;

const columnOptionsHtml = (columns, status) =>
  columnsOf(columns)
    .map((column) => `<option value="${esc(column.id)}"${column.id === status ? " selected" : ""}>${esc(column.name)}</option>`)
    .join("");

const priorityOptionsHtml = (priority) =>
  PRIORITIES.map(
    (candidate) => `<option value="${esc(candidate.id)}"${candidate.id === priority ? " selected" : ""}>${esc(candidate.label)}</option>`,
  ).join("");

const linkRowHtml = (row) =>
  row.route
    ? `<li class="issue-link"><a href="${esc(hashFromRoute(row.route))}">${esc(row.label)}</a></li>`
    : `<li class="issue-link"><code title="${esc(row.title || row.label)}">${esc(row.label)}</code></li>`;

const linksHtml = (rows) =>
  rows.length
    ? `<ul class="issue-links">${rows.map(linkRowHtml).join("")}</ul>`
    : `<p class="sub">Nothing linked yet. A dispatch links the workspace and the conversation it made.</p>`;

/**
 * The rail.
 *
 * Every control here writes one field, so each one is the smallest verb that
 * says what it means: `issues.close`/`issues.reopen` for the state,
 * `issues.update` for the column, the labels and the priority, and
 * `issues.assign` for who holds it. The page never sends a whole record.
 */
export function issueRailHtml(issue, context) {
  const { columns, links, labelsDraft, busy } = context;
  const assigneeLink = issue.assignee && actorHref(issue.assignee, context);
  return `<aside class="issue-rail" aria-label="About this issue">
    ${stateControlHtml(issue, busy)}
    ${railSection("Column", selectRow("issue-status", "Column", columnOptionsHtml(columns, issue.status), busy))}
    ${railSection("Labels", `<input id="issue-labels" type="text" autocomplete="off" placeholder="bug, ui" value="${esc(labelsDraft)}"${busy ? " disabled" : ""} />
      <p class="sub">Comma separated. Enter saves.</p>`)}
    ${railSection("Priority", selectRow("issue-priority", "Priority", priorityOptionsHtml(issue.priority), busy))}
    ${railSection("Assignee", `${assigneeLink
      ? `<div class="issue-assignee-current">${actorIdentityHtml(issue.assignee, context, { icon: true })}</div>` : ""}
      <button class="btn issue-assign-open" type="button" data-issue-assign="${esc(issue.id)}"${busy ? " disabled" : ""}>${assigneeLink ? "Change assignee" : assigneeHtml(issue.assignee, context)}</button>
      <p class="sub">Assigning hands the issue to an agent and starts it.</p>`)}
    ${railSection("Links", linksHtml(links))}
  </aside>`;
}

/** The whole page. */
export function issuePageHtml(issue, context) {
  return `<div class="issue-page">
    <div class="issue-page-main">
      ${issueHeadHtml(issue, context)}
      ${issueBodyHtml(issue, context.refLinks)}
      ${issueAttachmentsHtml(issue.attachments)}
      ${timelineHtml(context.rows, context)}
      ${composerHtml(context.draft, context.sending, context.attachable, context.hasFiles)}
    </div>
    ${issueRailHtml(issue, context)}
  </div>`;
}

/** An issue this device cannot answer for yet — never opened here, or gone. */
export const issueMissingHtml = () => `<div class="empty gone"><h2>This issue is not here</h2>
  <p>It may belong to another project, or it may have been read on another machine.</p></div>`;
