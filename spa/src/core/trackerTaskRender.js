// The task page, as HTML: the task, its timeline, the composer, and the rail
// of everything about it that can be changed.
//
// The rail starts with editable details and links; the close or reopen action
// sits at the bottom. Assigning sits low because it starts an agent, and a
// control that starts work should not be the first thing under a thumb.
//
// Pure: HTML in, no DOM, no app imports. core/trackerTaskPage.js mounts it.

import { esc } from "./text.js";
import { markdownHtml } from "./markdown.js";
import { hashFromRoute } from "./router.js";
import { columnsOf, PRIORITIES, stateLabel } from "./trackerModel.js";
import { taskAvatarHtml } from "./taskAvatar.js";
import { actorIdentityHtml, actorHref } from "./trackerIdentity.js";
import { watchButtonHtml } from "./watchToggle.js";
import { eventSentence } from "./trackerTimeline.js";
import { taskUnreadKey } from "./trackerUnread.js";
import { ageHtml, ageText, assigneeHtml, labelsHtml, numberHtml, stateMarkHtml } from "./trackerChips.js";
import { composerPartIds } from "./composer.js";
import { attachmentListHtml } from "./attachmentTiles.js";
import { ICON_PAPERCLIP } from "./icons.js";
import { fieldTraits } from "./fieldTraits.js";

/** The head: what the task is called, and the two facts that are independent
 *  of each other — is it still open, and where does it stand on the board.
 *
 *  `watch` comes from the cached task. A cold page offers the control before
 *  this device has greeted. */
export const taskHeadHtml = (task, { watch = null } = {}) => `<header class="task-page-head">
    <div class="task-page-marks">
      ${stateMarkHtml(task)}<span class="task-page-state">${esc(stateLabel(task.state))}</span>
      ${numberHtml(task)}
      ${ageHtml(task.updated_at)}
      ${watch ? watchButtonHtml(watch) : ""}
    </div>
    <h1 class="task-page-title">${esc(task.title)}</h1>
    ${task.labels?.length ? `<div class="task-page-labels">${labelsHtml(task.labels)}</div>` : ""}
  </header>`;

/** Markdown written on this task, read from where the page stands: the project
 *  a `#42` in it means, and every actor the task carries, departed ones too
 *  (#229). */
const taskMarkdownHtml = (text, context, taskItems = false, taskLabelScope) =>
  markdownHtml(text, {
    place: { deviceId: context.deviceId, projectId: context.projectId },
    identities: context.identities,
    taskItems,
    taskLabelScope,
  });

const taskMarkdownScope = (context, kind, id) => JSON.stringify([context.deviceId, context.projectId, kind, id]);

/** The body, as markdown. A task with an empty body says so rather than
 *  leaving a gap a reader has to interpret. */
export const taskBodyHtml = (task, context = {}) =>
  task.body
    // markdownHtml escapes all input before adding its fixed safe tag set.
    ? `<div class="task-page-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ taskMarkdownHtml(task.body, context, true, taskMarkdownScope(context, "body", task.id))}</div>`
    : `<p class="task-page-body empty">No description.</p>`;

/**
 * The files filed with the task (#57).
 *
 * Deliberately the conversation's own tiles (core/attachmentTiles.js), down to
 * `data-attachment-path`: one attachment story in this client rather than two.
 * That is what lets `wireThreadAttachments` fill these and open them in the one
 * lightbox — the loading, the refusal and the wire-went-away deferral are all
 * already written, and a second copy of them would be a second set of answers
 * about one failure.
 *
 * `src` is left empty and filled by that wiring, because the page is a string
 * and the bytes are a round trip away.
 */
export const taskAttachmentsHtml = (attachments) =>
  attachmentListHtml(attachments, { className: "task-page-attachments" });

const whenHtml = (row) => (row.at ? `<span class="task-when" title="${esc(row.at)}">${esc(ageText(row.at))}</span>` : "");

const reviewVerdictHtml = (opinion) => ({
  approve: "<span>Approved</span>",
  request_changes: "<span>Requested changes</span>",
})[opinion?.verdict] || "";
const reviewAnchorHtml = (anchor) => anchor
  ? `<button class="btn" type="button" data-review-anchor="${esc(JSON.stringify(anchor))}">` +
    `${esc(anchor.path)} · ${esc(anchor.side)} line ${esc(anchor.line)}</button>` : "";
const snapshotLabel = (snapshotId, snapshots) => {
  if (!snapshotId) return "";
  const snapshot = (snapshots || []).find((saved) => saved.id === snapshotId);
  return snapshot?.number ? `Snapshot ${esc(snapshot.number)}` : "Earlier snapshot";
};
const reviewMetadataHtml = (row, context) => {
  const snapshotId = row.opinion?.snapshot_id || row.anchor?.snapshot_id;
  const verdict = reviewVerdictHtml(row.opinion);
  const label = snapshotLabel(snapshotId, context.reviewSnapshots);
  const snapshot = label ? `<span>${label}</span>` : "";
  const place = reviewAnchorHtml(row.anchor);
  const reply = row.replyTo ? `<span>Reply to ${esc(row.replyTo)}</span>` : "";
  if (!snapshot && !verdict && !place && !reply) return "";
  return `<div class="task-review-comment-meta">${snapshot}${verdict}${place}${reply}</div>`;
};

/// The id an action line in a conversation lands on: a comment is linked as
/// `#comment-<id>` (core/trackerActionLine.js), so the row has to answer to it.
const commentHtml = (row, context) => `<li class="task-entry task-comment${row.mentionsUser ? " task-comment-mentioned" : ""}" id="comment-${esc(row.key)}">
    ${taskAvatarHtml(row.actor, context)}
    <div class="task-comment-card">
      <div class="task-entry-head"><strong>${actorIdentityHtml(row.actor, context)}</strong>${whenHtml(row)}</div>
      ${reviewMetadataHtml(row, context)}
      <div class="task-comment-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ taskMarkdownHtml(row.body, context, false, taskMarkdownScope(context, "comment", row.key))}</div>
      ${attachmentListHtml(row.attachments, { className: "task-comment-attachments" })}
      ${row.anchor || row.opinion || row.replyTo ? `<button class="btn" type="button" data-review-reply="${esc(row.key)}">Reply</button>` : ""}
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

const eventHtml = (row, context) => `<li class="task-entry task-event">
    <span class="task-event-dot" aria-hidden="true"></span>
    <span class="task-event-text"><strong>${actorIdentityHtml(row.actor, context, { icon: true })}</strong> ${eventDetailHtml(row, context)}</span>
    ${whenHtml(row)}
  </li>`;

/** One row, with the unread divider ahead of it when the reader's new
 *  activity starts there. */
const timelineRowHtml = (row, context) => {
  const line = context.unreadFrom != null && context.unreadFrom === taskUnreadKey(row.key)
    ? '<li class="thread-unread-line task-unread-line" role="separator"><span>New</span></li>' : "";
  return line + (row.type === "comment" ? commentHtml(row, context) : eventHtml(row, context));
};

/** The timeline as its list and one part per row, keyed by the row's record,
 *  for a page that patches rows (core/partPatch.js): a push adds its row and
 *  leaves every row already on screen, pictures and all, where it was. */
export function timelineParts(rows, context) {
  if (!rows.length) return { frame: `<p class="empty task-empty">Nothing has happened on this task yet.</p>`, rows: [] };
  return {
    frame: '<ul class="task-timeline"></ul>',
    rows: rows.map((row) => ({ name: row.key, html: timelineRowHtml(row, context) })),
  };
}

/** The timeline: comments and events interleaved, ascending, in the order the
 *  bridge answered them. Never re-sorted here — see core/trackerTimeline.js. */
export function timelineHtml(rows, context) {
  const { frame, rows: parts } = timelineParts(rows, context);
  return parts.length ? frame.replace("</ul>", `${parts.map((part) => part.html).join("")}</ul>`) : frame;
}

/// The ids `mountComposerAttachments` reads on the comment box. The textarea
/// keeps the id it always had, so everything already addressing it still does.
export const COMMENT_INPUT_ID = "task-comment";
const commentParts = composerPartIds(COMMENT_INPUT_ID);

/**
 * The comment box.
 *
 * The send press is enabled by a draft OR by a tray with something in it: a
 * comment that is only a screenshot is a comment.
 */
/// The paperclip, the hidden picker and the drop mask.
export const commentAttachHtml = () => `<div class="composer-bar">
      <div class="composer-actions">
        <input type="file" id="${commentParts.file}" class="composer-file" multiple hidden>
        <button type="button" class="composer-attach" id="${commentParts.attach}" aria-label="Attach files" title="Attach files">${ICON_PAPERCLIP}</button>
      </div>
    </div>
    <div class="composer-dropmask" aria-hidden="true"><span>Drop to attach</span></div>`;

/// Whether the send press can be pressed. A comment that is only a screenshot
/// is a comment, so a tray with something in it is as good as a draft.
export const canComment = (draft, busy, hasFiles) => !busy && (Boolean(draft.trim()) || hasFiles);

/// What the send press says.
export const commentSendLabel = (busy) => (busy ? "sending…" : "Comment");

const commentFieldHtml = (draft, busy) =>
  `<textarea id="${COMMENT_INPUT_ID}" rows="3" ${fieldTraits("prose")} placeholder="Comment on this task"${busy ? " disabled" : ""}>${esc(draft)}</textarea>`;

/// The tray the attached files sit in, above the box.
export const commentTrayHtml = () => `<div class="composer-tray" id="${commentParts.tray}" hidden></div>`;

/// The box in its frame. The frame is there on every bridge, so a greeting
/// keeps the same textarea and tray as the page repaints under the reader (#153).
const commentBoxHtml = (draft, busy) => `${commentTrayHtml()}
    <div class="task-comment-field composer attachable task-comment-box">
      ${commentFieldHtml(draft, busy)}
      ${commentAttachHtml()}
    </div>`;

export const composerHtml = (draft, busy, hasFiles = false) => `<form class="task-composer" data-task-composer>
    <label class="sr-only" for="${COMMENT_INPUT_ID}">Comment on this task</label>
    ${commentBoxHtml(draft, busy)}
    <div class="row task-composer-row">
      <button class="btn primary" type="submit"${canComment(draft, busy, hasFiles) ? "" : " disabled"}>${commentSendLabel(busy)}</button>
    </div>
  </form>`;

// ---- the rail ---------------------------------------------------------------

const railSection = (title, inner) => `<section class="task-rail-section"><h2>${esc(title)}</h2>${inner}</section>`;

/** Close and reopen, and nothing else on this control. Closing a task stops
 *  nothing and starts nothing — it says the work is over — so the button says
 *  only that. */
const stateControlHtml = (task, busy) => `<div class="task-rail-action">
  <button class="btn" type="button" data-task-state${busy ? " disabled" : ""}>${task.state === "closed" ? "Reopen task" : "Close task"}</button>
</div>`;

const selectSection = (id, title, optionsHtml, busy) => `<section class="task-rail-section">
  <h2 id="${esc(id)}-label">${esc(title)}</h2>
  <select id="${esc(id)}" aria-labelledby="${esc(id)}-label"${busy ? " disabled" : ""}>${optionsHtml}</select>
</section>`;

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
    ? `<li class="task-link"><a href="${esc(hashFromRoute(row.route))}">${esc(row.label)}</a></li>`
    : `<li class="task-link"><code title="${esc(row.title || row.label)}">${esc(row.label)}</code></li>`;

const linksHtml = (rows) =>
  rows.length
    ? `<ul class="task-links">${rows.map(linkRowHtml).join("")}</ul>`
    : `<p class="sub">Nothing linked yet. A dispatch links the workspace and the conversation it made.</p>`;

/**
 * The rail.
 *
 * Every control here writes one field, so each one is the smallest verb that
 * says what it means: `tasks.close`/`tasks.reopen` for the state,
 * `tasks.update` for the column, the labels and the priority, and
 * `tasks.assign` for who holds it. The page never sends a whole record.
 */
export function taskRailHtml(task, context) {
  const { columns, links, labelsDraft, busy } = context;
  const assigneeLink = task.assignee && actorHref(task.assignee, context);
  return `<aside class="task-rail" aria-label="About this task">
    ${selectSection("task-status", "Status", columnOptionsHtml(columns, task.status), busy)}
    ${railSection("Labels", `<input id="task-labels" type="text" ${fieldTraits("identifier")} placeholder="bug, ui" value="${esc(labelsDraft)}"${busy ? " disabled" : ""} />
      <p class="sub">Comma separated.</p>`)}
    ${selectSection("task-priority", "Priority", priorityOptionsHtml(task.priority), busy)}
    ${railSection("Assignee", `${assigneeLink
      ? `<div class="task-assignee-current">${actorIdentityHtml(task.assignee, context, { icon: true })}</div>` : ""}
      <button class="btn task-assign-open" type="button" data-task-assign="${esc(task.id)}"${busy ? " disabled" : ""}>${assigneeLink ? "Change assignee" : assigneeHtml(task.assignee, context)}</button>`)}
    ${railSection("Links", linksHtml(links))}
    ${stateControlHtml(task, busy)}
  </aside>`;
}

/** The page's frame: the column the task reads down, which the rail follows. */
export const TASK_PAGE_FRAME = '<div class="task-page"><div class="task-page-main"></div></div>';

/**
 * The page as parts, in order, for a surface that repaints only the parts
 * that changed (core/partPatch.js): `main` fills the frame's column, `rail`
 * follows it, and `timeline` fills the list the `timeline` part stands up.
 *
 * The comment box is painted once per frame and never again: its draft,
 * whether it is sending, whether it can be sent and whether it takes files are
 * all updated in place by the page, so the textarea is one node per mount and
 * nothing stands up a new one under the reader (#153).
 */
export function taskPageParts(task, context) {
  const timeline = timelineParts(context.rows, context);
  return {
    main: [
      { name: "head", html: taskHeadHtml(task, context) },
      { name: "body", html: taskBodyHtml(task, context) },
      { name: "attachments", html: taskAttachmentsHtml(task.attachments) },
      { name: "review", html: '<section data-task-review hidden></section>', key: "review" },
      { name: "timeline", html: timeline.frame },
      {
        name: "composer",
        html: composerHtml(context.draft, context.sending, context.hasFiles),
        key: "composer",
      },
    ],
    rail: [{ name: "rail", html: taskRailHtml(task, context) }],
    timeline: timeline.rows,
  };
}

/** The whole page, as one string. */
export function taskPageHtml(task, context) {
  return `<div class="task-page">
    <div class="task-page-main">
      ${taskHeadHtml(task, context)}
      ${taskBodyHtml(task, context)}
      ${taskAttachmentsHtml(task.attachments)}
      <section data-task-review hidden></section>
      ${timelineHtml(context.rows, context)}
      ${composerHtml(context.draft, context.sending, context.hasFiles)}
    </div>
    ${taskRailHtml(task, context)}
  </div>`;
}

/** A task this device cannot answer for yet — never opened here, or gone. */
export const taskMissingHtml = () => `<div class="empty gone"><h2>This task is not here</h2>
  <p>It may belong to another project, or it may have been read on another machine.</p></div>`;
