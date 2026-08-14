import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";
import { completionReportSections } from "./agentRailModel.js";
import {
  autoGrow,
  composerHtml,
  formatAttachmentSize,
  isImageAttachment,
  mountComposerAttachments,
} from "./composer.js";

const EVENT_META = {
  session_started: { label: "Agent session started", icon: "▶" },
  session_ended: { label: "Agent session ended", icon: "■" },
  run_started: { label: "Run started", icon: "▶" },
  run_failed: { label: "Agent reported failure", icon: "×", tone: "blocked" },
  blocked: { label: "Agent reported a blocker", icon: "!", tone: "blocked" },
  review_blocked: { label: "Review blocked", icon: "!", tone: "blocked" },
  idle_unreported: { label: "Agent went idle without reporting", icon: "…", tone: "blocked" },
  done: { label: "Agent reported done", icon: "✓", tone: "success" },
  revision_created: { label: "Revision created", icon: "↻" },
  approved: { label: "Issue ready", icon: "✓", tone: "success" },
  stage_approved: { label: "Stage approved", icon: "✓", tone: "success" },
  stage_started: { label: "Stage implementation started", icon: "▶" },
  implementation_started: { label: "Implementation started", icon: "▶" },
  worktree_reused: { label: "Implementation worktree reused", icon: "↻", tone: "success" },
  worktree_recreated: { label: "Implementation worktree recreated", icon: "↻", tone: "success" },
  worktree_deleted: { label: "Implementation worktree deleted", icon: "×", tone: "blocked" },
  recovery_started: { label: "Verified recovery started", icon: "▶" },
  recovery_succeeded: { label: "Verified recovery succeeded", icon: "✓", tone: "success" },
  recovery_failed: { label: "Verified recovery failed", icon: "×", tone: "blocked" },
  stage_completed: { label: "Stage completed", icon: "✓", tone: "success" },
  stage_invalidated: { label: "Stage marked incomplete", icon: "!", tone: "blocked" },
  implementation_archived: { label: "Implementation archived", icon: "■" },
  // A pass that only orders the diff for review: it carries no tone, because
  // it asks the reviewer for nothing.
  triaged: { label: "Diff ordered for review", icon: "≡" },
  // The reviewer disagreed with where the pass put something. Also toneless:
  // it is a note to the agent, not a call on anybody.
  triage_overridden: { label: "Review order corrected", icon: "≠" },
  committed: { label: "Changes committed", icon: "◆", tone: "success" },
  pushed: { label: "Changes pushed", icon: "↑", tone: "success" },
  merged: { label: "Changes merged", icon: "⌁", tone: "success" },
  abandoned: { label: "Abandoned", icon: "×", tone: "blocked" },
};

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

function ordinal(day) {
  const lastTwoDigits = day % 100;
  if (lastTwoDigits >= 11 && lastTwoDigits <= 13) return `${day}th`;
  return `${day}${({ 1: "st", 2: "nd", 3: "rd" })[day % 10] || "th"}`;
}

function calendarDayNumber(date) {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / DAY_MS;
}

export function formatRelativeDate(value, nowValue = new Date()) {
  const date = value instanceof Date ? value : new Date(value);
  const now = nowValue instanceof Date ? nowValue : new Date(nowValue);
  if (Number.isNaN(date.getTime()) || Number.isNaN(now.getTime())) return "";

  const elapsed = now.getTime() - date.getTime();
  if (elapsed >= 0 && elapsed < MINUTE_MS) return "Just now";
  if (elapsed >= 0 && elapsed < HOUR_MS) {
    const minutes = Math.floor(elapsed / MINUTE_MS);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  if (elapsed >= 0 && elapsed < DAY_MS) {
    const hours = Math.floor(elapsed / HOUR_MS);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }

  const daysAgo = calendarDayNumber(now) - calendarDayNumber(date);
  if (daysAgo === 1) {
    const options = date.getMinutes()
      ? { hour: "numeric", minute: "2-digit" }
      : { hour: "numeric" };
    const time = date.toLocaleTimeString("en-US", options).replace(/\s/g, "").toLowerCase();
    return `Yesterday at ${time}`;
  }
  if (daysAgo > 1 && daysAgo < 7) {
    return date.toLocaleDateString("en-US", { weekday: "long" });
  }

  const month = date.toLocaleDateString("en-US", { month: "long" });
  const label = `${month} ${ordinal(date.getDate())}`;
  return date.getFullYear() === now.getFullYear() ? label : `${label}, ${date.getFullYear()}`;
}

function timeHtml(createdAt) {
  if (!createdAt) return "";
  const label = formatRelativeDate(createdAt);
  if (!label) return "";
  return `<time datetime="${esc(createdAt)}">${esc(label)}</time>`;
}

// Client half of the thread cursor: the detail polls (plan.get / run.get every
// 1.6s) would otherwise re-ship the whole forever-growing conversation over
// E2EE on every tick. The cache holds one entity's accumulated items, tells the
// caller which cursor to send, and folds each delta back into a full thread for
// rendering. A `thread_total` that disagrees with what it holds (bridge
// restart, entity swap, dropped delta) resets it to a full refetch.
export function createThreadCache() {
  let accumulatedItems = [];

  // The bridge bumps `updated_sequence` (drawn from the same counter as
  // `sequence`) when it mutates a message in place — marking it seen,
  // resolving it with a revision — so the cursor must cover the newest
  // counter value any held item has touched, not just the newest creation.
  const itemCursorSequence = (item) =>
    Math.max(item.data?.sequence || 0, item.data?.updated_sequence || 0);

  const lastHeldSequence = () =>
    accumulatedItems.reduce((highest, item) => Math.max(highest, itemCursorSequence(item)), 0);

  return {
    // Extra params for the next plan.get / run.get: the last sequence held, or
    // nothing when a full fetch is needed (first load, or after a reset).
    cursorParam() {
      return accumulatedItems.length ? { thread_after_sequence: lastHeldSequence() } : {};
    },
    // Fold a polled thread payload into the cache and return a thread whose
    // `items` is the complete accumulated list. Never mutates the payload.
    absorb(threadPayload) {
      if (!threadPayload) {
        accumulatedItems = [];
        return threadPayload;
      }
      const arrivedItems = threadPayload.items || [];
      if (threadPayload.thread_total == null) {
        // An uncursored (full) response is authoritative: replace, don't merge.
        accumulatedItems = [...arrivedItems];
        return { ...threadPayload, items: accumulatedItems };
      }
      // Keyed by creation sequence so a replay never grows the list, while an
      // arrived copy replaces the held one — the bridge re-ships an item
      // exactly when it holds newer state (seen, resolved) for it.
      const mergedBySequence = new Map(accumulatedItems.map((item) => [item.data?.sequence, item]));
      for (const arrived of arrivedItems) {
        mergedBySequence.set(arrived.data?.sequence, arrived);
      }
      const merged = [...mergedBySequence.values()].sort(
        (a, b) => (a.data?.sequence || 0) - (b.data?.sequence || 0),
      );
      if (merged.length !== threadPayload.thread_total) {
        // Gap or shrink: render what we have this tick, but drop the cache so
        // the next poll refetches the full thread and self-heals.
        accumulatedItems = [];
        return { ...threadPayload, items: merged };
      }
      accumulatedItems = merged;
      return { ...threadPayload, items: accumulatedItems };
    },
    reset() {
      accumulatedItems = [];
    },
  };
}

export function currentRevisionId(thread, artifact) {
  const revisions = (thread && thread.revisions) || [];
  return [...revisions].reverse().find((revision) => revision.artifact === artifact)?.id || null;
}

function anchorLabel(anchor) {
  if (!anchor) return "";
  const path = anchor.path || (anchor.artifact === "plan" ? "plan" : "diff");
  const start = anchor.line_start;
  const end = anchor.line_end;
  const lines = start == null ? "" : start === end ? `:${start}` : `:${start}-${end}`;
  const heading = anchor.heading_path && anchor.heading_path.length ? ` · ${anchor.heading_path.join(" › ")}` : "";
  return `<div class="thread-anchor">${esc(path + lines + heading)}${anchor.snippet ? ` · “${esc(anchor.snippet.replace(/\s+/g, " ").slice(0, 120))}”` : ""}</div>`;
}

function harnessLabel(thread, override) {
  const raw = override || (thread && thread.sessions && thread.sessions.at(-1)?.provider) || "Agent";
  if (raw === "codex" || raw === "Codex CLI") return "Codex";
  if (raw === "claude" || raw === "Claude") return "Claude Code";
  return raw;
}

function linkLocation(link) {
  if (link.kind !== "file") {
    return link.path || link.implementation_id || link.run_id || link.worktree_id || link.sha || link.recovery_id || "Open";
  }
  const start = link.line_start;
  const end = link.line_end;
  const lines = start == null ? "" : start === end || end == null ? `:${start}` : `:${start}-${end}`;
  return `${link.path || "file"}${lines}`;
}

function linksHtml(links) {
  if (!links || !links.length) return "";
  return `<div class="thread-references">${links
    .map((link) => {
      const attributes = [
        `data-kind="${esc(link.kind || "")}"`,
        link.path ? `data-path="${esc(link.path)}"` : "",
        link.issue_id ? `data-issue-id="${esc(link.issue_id)}"` : "",
        link.plan_id ? `data-plan-id="${esc(link.plan_id)}"` : "",
        link.stage_id ? `data-stage-id="${esc(link.stage_id)}"` : "",
        link.implementation_id ? `data-implementation-id="${esc(link.implementation_id)}"` : "",
        link.run_id ? `data-run-id="${esc(link.run_id)}"` : "",
        link.worktree_id ? `data-worktree-id="${esc(link.worktree_id)}"` : "",
        link.sha ? `data-sha="${esc(link.sha)}"` : "",
        link.recovery_id ? `data-recovery-id="${esc(link.recovery_id)}"` : "",
        link.line_start != null ? `data-line-start="${Number(link.line_start)}"` : "",
        link.line_end != null ? `data-line-end="${Number(link.line_end)}"` : "",
      ].filter(Boolean).join(" ");
      return `<button type="button" class="thread-reference" ${attributes}>${esc(linkLocation(link))}</button>`;
    })
    .join("")}</div>`;
}

/// The files a message came with.
///
/// An image is shown, not linked: the reason to attach a screenshot is that
/// looking at it IS the message, and a chip reading "screenshot.png" makes the
/// reader click to find out what they were told. Everything else is a chip that
/// downloads, since the browser has nothing useful to do with a tarball.
///
/// `src` is left empty here and filled by [`wireThreadAttachments`] — the
/// timeline is a string, and the bytes are a round trip away.
function attachmentsHtml(attachments) {
  if (!attachments || !attachments.length) return "";
  return `<div class="thread-attachments">${attachments
    .map((attachment) => {
      const path = esc(attachment.path || "");
      const name = esc(attachment.name || attachment.path || "file");
      if (isImageAttachment(attachment.mime)) {
        return `<figure class="thread-attachment-figure">
          <img class="thread-attachment-image" data-attachment-path="${path}" alt="${name}">
          <figcaption>${name}</figcaption>
        </figure>`;
      }
      return `<button type="button" class="thread-attachment" data-attachment-path="${path}" data-attachment-name="${name}">
        <span class="thread-attachment-name">${name}</span>
        <span class="thread-attachment-size">${esc(formatAttachmentSize(attachment.size))}</span>
      </button>`;
    })
    .join("")}</div>`;
}

function messageHtml(message, agentLabel = "Agent") {
  const user = message.role === "user";
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  // `done` is message metadata, not a presentation type. A done-flagged send
  // follows the timeline's done event and otherwise renders like every message.
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}">
    <span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>
    <div class="thread-comment-card">
      <div class="thread-message-head"><span><strong>${user ? "You" : esc(agentLabel)}</strong> commented ${timeHtml(message.created_at)}</span>${status}</div>
      ${anchorLabel(message.anchor)}
      ${message.body ? `<div class="thread-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(message.body)}</div>` : ""}
      ${attachmentsHtml(message.attachments)}
      ${linksHtml(message.links)}
    </div>
  </article>`;
}

/// The agent's handoff, as a card.
///
/// `done` is asked for a completion report and the event is the whole record of
/// it (there is no companion message any more), so the report renders where the
/// event does: the critical files, the decisions a reviewer would otherwise
/// reverse-engineer, the risks, and what was deliberately left alone. Every
/// line is the agent's words — escaped.
function completionReportHtml(report) {
  const sections = completionReportSections(report);
  if (!sections.length) return "";
  return `<div class="completion-report">${sections
    .map(
      (section) =>
        `<div class="completion-section"><div class="completion-title">${esc(section.title)}</div>
        <ul>${section.items.map((item) => `<li>${esc(item)}</li>`).join("")}</ul></div>`,
    )
    .join("")}</div>`;
}

function eventHtml(event, agentLabel = "Agent") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  const label = meta.label.replace(/^Agent\b/, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.event !== "done" && event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}${completionReportHtml(event.completion_report)}${linksHtml(event.links)}</div>
  </div>`;
}

/// The timeline: what was said, and what happened.
///
/// Working time and the diffstat are NOT here. They are facts about the branch
/// or issue rather than about anything anyone said, they are true wherever you
/// are standing in the work, and they change every second — so they live on the
/// toolbar (core/toolbar.js) and the conversation keeps its own record: the
/// messages, the events, and whether the agent has read you.
function timelineHtml(items, agentLabel) {
  return items.flatMap((item) => {
    if (item.type !== "message") return [eventHtml(item.data || {}, agentLabel)];
    const message = item.data || {};
    // Old bridges persisted the noisy structured handoff as a chat message.
    if (message.source === "completion" && String(message.body || "").includes("Completion report")) return [];
    return [messageHtml(message, agentLabel)];
  });
}

// The plan composer's historical ids/copy, kept as the `composer: true`
// defaults so existing callers are unchanged.
const PLAN_COMPOSER_DEFAULTS = {
  inputId: "planthreadinput",
  sendId: "planthreadsend",
  hintId: "planthreadhint",
  placeholder: "Send a message to the planning agent…",
};

// `composer` is falsy (no composer), `true` (plan defaults), or an object
// overriding ids/placeholder/attachable — caller-scoped ids let two thread
// surfaces (the plan review and the run diff) each mount a composer on one page
// without colliding.
function threadComposerHtml(composer) {
  if (!composer) return "";
  return composerHtml({
    ...PLAN_COMPOSER_DEFAULTS,
    ...(composer === true ? {} : composer),
  });
}

/// The conversation's own status + lifecycle strip.
///
/// The surface bar carries tabs and a branch and nothing else, so where a run or
/// an issue STANDS, and what you can do about it, belong to the thread that
/// records how it got there: the chip rides the conversation's title, and the
/// verbs sit at the end of the timeline, right above the box you would reply in.
function statusChipHtml(status) {
  if (!status || !status.label) return "";
  return `<span class="chip thread-state ${esc(status.cls || "")}">${esc(status.label)}</span>`;
}

function threadActionsHtml(actionsId) {
  return actionsId ? `<div class="thread-actions" id="${esc(actionsId)}"></div>` : "";
}

export function threadHtml(thread, options = {}) {
  const agentLabel = harnessLabel(thread, options.agentLabel);
  const sourceItems = (thread && thread.items) || [];
  const initialMessage = String(options.initialMessage || "").trim();
  const hasInitialMessage = sourceItems.some(
    (item) => item.type === "message" && item.data?.role === "user" && String(item.data.body || "").trim() === initialMessage,
  );
  const items = initialMessage && !hasInitialMessage
    ? [{ type: "message", data: { role: "user", body: initialMessage, seen_at: "initial" } }, ...sourceItems]
    : sourceItems;
  const renderedItems = timelineHtml(items, agentLabel);
  const itemCount = renderedItems.length;
  // The timeline draws the avatar spine, and the messages sit in the gutter it
  // runs down. With nothing on the record there is neither, so the empty case
  // says so and the CSS drops both rather than ruling a line beside a sentence.
  const empty = itemCount ? "" : " is-empty";
  return `<section class="review-thread pane-col${empty}">
    <div class="thread-title"><span class="thread-title-text">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</span>${statusChipHtml(options.status)}</div>
    <div class="thread-items thread-timeline${empty}">${itemCount
      ? renderedItems.join("")
      : '<div class="thread-empty">No conversation yet.</div>'}</div>
    <div class="thread-revision-view" hidden></div>
    ${threadActionsHtml(options.actionsId)}
    ${threadComposerHtml(options.composer)}
  </section>`;
}

/** How near the end still counts as reading the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_BOTTOM_SLACK_PX = 32;

/// Paint a conversation with the reader's place kept.
///
/// The newest message is the one the human came for and it sits at the END, so
/// opening a thread lands at the bottom. Every surface then re-renders the whole
/// timeline on its poll, and writing innerHTML resets scrollTop — which is the
/// same lever, so both halves live here: a reader already at the end is carried
/// along with new messages, and a reader who scrolled up is left exactly where
/// they were rather than yanked back down mid-sentence.
///
/// `scroller` is the element that scrolls (the surfaces' `#tabbody`), which is
/// not always the element `paint` writes into — the issue surface paints a
/// wrapper inside it. With no scroller this is `paint()` and nothing else.
export function paintThreadKeepingPlace(scroller, paint) {
  if (!scroller) {
    paint();
    return;
  }
  // Nothing rendered yet means this paint is the open: the tab was just
  // selected, or a shell rebuild wiped the body under it.
  const opening = !scroller.querySelector(".review-thread");
  const previousScrollTop = scroller.scrollTop;
  const wasAtBottom =
    scroller.scrollHeight - scroller.clientHeight - previousScrollTop <= AT_BOTTOM_SLACK_PX;
  paint();
  if (!opening && !wasAtBottom) {
    scroller.scrollTop = previousScrollTop;
    return;
  }
  const toBottom = () => {
    scroller.scrollTop = scroller.scrollHeight;
  };
  toBottom();
  // Markdown and web fonts can settle a frame after the content lands, leaving
  // the open short of the newest message. Only the open re-pins: doing it on a
  // poll's repaint would fight a reader who scrolled away within that frame.
  if (opening && typeof requestAnimationFrame === "function") requestAnimationFrame(toBottom);
}

export function wireThreadRevisionLinks(root, loadRevision) {
  if (!root) return;
  root.querySelectorAll(".thread-revision-link").forEach((button) => {
    button.onclick = async () => {
      const thread = button.closest(".review-thread");
      const viewer = thread && thread.querySelector(".thread-revision-view");
      if (!viewer) return;
      viewer.hidden = false;
      viewer.innerHTML = `<div class="thread-revision-head">Loading ${esc(button.dataset.revision)}…</div>`;
      try {
        const revision = await loadRevision(button.dataset.revision);
        viewer.innerHTML = `<div class="thread-revision-head"><strong>${esc(revision.revision_id)}</strong><button type="button" aria-label="Close revision">×</button></div><pre>${esc(revision.contents || "")}</pre>`;
        viewer.querySelector("button").onclick = () => {
          viewer.hidden = true;
          viewer.innerHTML = "";
        };
      } catch (error) {
        viewer.innerHTML = `<div class="thread-revision-head">Could not load revision: ${esc(error.message || String(error))}</div>`;
      }
    };
  });
}

/// Attachment bytes already fetched, keyed by path.
///
/// Safe to hold forever within a session: an attachment is content-addressed
/// and immutable, so a path always means the same bytes. Worth holding, because
/// the surfaces re-render the whole timeline on every poll and a conversation
/// full of screenshots would otherwise re-fetch all of them every second and a
/// half. Bounded so a long session cannot grow without limit.
const attachmentDataUrls = new Map();
const ATTACHMENT_CACHE_MAX = 40;

function rememberAttachment(path, dataUrl) {
  if (attachmentDataUrls.size >= ATTACHMENT_CACHE_MAX) {
    attachmentDataUrls.delete(attachmentDataUrls.keys().next().value);
  }
  attachmentDataUrls.set(path, dataUrl);
}

/// Fill the images a rendered timeline is waiting on, and make the file chips
/// download what they name.
///
/// `load(path)` resolves the bridge's `thread.attachment` payload
/// (`{mime, content_b64}`).
export function wireThreadAttachments(root, load) {
  if (!root) return;
  const dataUrlFor = async (path) => {
    if (attachmentDataUrls.has(path)) return attachmentDataUrls.get(path);
    const attachment = await load(path);
    const dataUrl = `data:${attachment.mime || "application/octet-stream"};base64,${attachment.content_b64 || ""}`;
    rememberAttachment(path, dataUrl);
    return dataUrl;
  };

  root.querySelectorAll("img.thread-attachment-image").forEach((image) => {
    const path = image.dataset.attachmentPath;
    if (!path || image.getAttribute("src")) return;
    dataUrlFor(path).then(
      (dataUrl) => {
        image.setAttribute("src", dataUrl);
      },
      () => {
        // A picture that will not load says so where the picture would be,
        // rather than leaving a silent gap in the conversation.
        image.closest(".thread-attachment-figure")?.classList.add("unavailable");
      },
    );
  });

  root.querySelectorAll("button.thread-attachment").forEach((chip) => {
    chip.onclick = async () => {
      const path = chip.dataset.attachmentPath;
      if (!path) return;
      const dataUrl = await dataUrlFor(path);
      const link = root.ownerDocument.createElement("a");
      link.href = dataUrl;
      link.download = chip.dataset.attachmentName || "attachment";
      link.click();
    };
  });
}

export function wireThreadLinks(root, openLink) {
  if (!root) return;
  root.querySelectorAll(".thread-reference").forEach((button) => {
    button.onclick = () => {
      const link = { kind: button.dataset.kind };
      if (button.dataset.path) link.path = button.dataset.path;
      if (button.dataset.issueId) link.issue_id = button.dataset.issueId;
      if (button.dataset.planId) link.plan_id = button.dataset.planId;
      if (button.dataset.stageId) link.stage_id = button.dataset.stageId;
      if (button.dataset.implementationId) link.implementation_id = button.dataset.implementationId;
      if (button.dataset.runId) link.run_id = button.dataset.runId;
      if (button.dataset.worktreeId) link.worktree_id = button.dataset.worktreeId;
      if (button.dataset.sha) link.sha = button.dataset.sha;
      if (button.dataset.recoveryId) link.recovery_id = button.dataset.recoveryId;
      if (button.dataset.lineStart) link.line_start = Number(button.dataset.lineStart);
      if (button.dataset.lineEnd) link.line_end = Number(button.dataset.lineEnd);
      openLink(link);
    };
  });
}

/// Wire a thread composer's submit path: draft restore, re-entry guard,
/// in-place button restore, and Cmd/Ctrl+Enter.
///
/// Shared because the plan and diff composers are the same gesture and drifted
/// apart once: Cmd+Enter reaches submit without the button's native disabled
/// gate, so a composer that only disables the button double-posts on the retry
/// a wedged-looking box invites. Restoring the button here — before any
/// repaint — keeps that true even when the caller's rebuild is frozen.
///
/// `onSubmit(body, attachments)` does the transport and resolves when the post
/// has landed. `upload` (with the `readAttachments`/`writeAttachments` draft
/// pair) turns the box into one that takes files; without it the composer is
/// the plain text box it always was.
/// The send button's word. It wraps its label so a busy state can rewrite the
/// word without wiping the icon beside it; an older composer without the span
/// is still driven directly.
const sendLabel = (button) => button.querySelector(".composer-send-label") || button;

export function wireThreadComposer(root, { ids, onSubmit, readDraft, writeDraft, onError, afterSubmit, upload, readAttachments, writeAttachments }) {
  if (!root) return;
  const input = root.querySelector(`#${ids.input}`);
  const send = root.querySelector(`#${ids.send}`);
  const hint = ids.hint ? root.querySelector(`#${ids.hint}`) : null;
  if (!input || !send) return;

  const say = (message) => {
    if (hint) hint.textContent = message;
  };
  const tray = upload
    ? mountComposerAttachments(root, {
        ids,
        upload,
        readAttachments,
        writeAttachments,
        onError: say,
      })
    : null;

  input.value = readDraft();
  input.oninput = () => {
    writeDraft(input.value);
    say("");
  };
  const fitToText = autoGrow(input);

  const submit = async () => {
    // A send is already in flight: the keyboard path has no disabled gate.
    if (send.disabled) return;
    if (tray && tray.busy()) {
      // The message names its files by path, so posting before they land would
      // hand the agent a reference to bytes that do not exist yet.
      say("A file is still attaching…");
      return;
    }
    const body = input.value.trim();
    // A file on its own is a message; words are only required when there is
    // nothing else being sent.
    if (!body && (!tray || tray.isEmpty())) {
      say("Type a message first.");
      input.focus();
      return;
    }
    send.disabled = true;
    sendLabel(send).textContent = "sending…";
    try {
      const result = await onSubmit(body, tray ? tray.attachments() : []);
      writeDraft("");
      input.value = "";
      fitToText();
      if (tray) tray.clear();
      send.disabled = false;
      sendLabel(send).textContent = "Send";
      if (afterSubmit) afterSubmit(result);
    } catch (error) {
      // The text and the files stay put: a failed send must never cost the user
      // their words, and re-picking the files would be worse.
      send.disabled = false;
      sendLabel(send).textContent = "Send";
      if (onError) onError(error);
    }
  };

  send.onclick = submit;
  input.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  };
}

