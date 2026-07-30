import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";

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
  approved: { label: "Plan approved", icon: "✓", tone: "success" },
  stage_approved: { label: "Stage approved", icon: "✓", tone: "success" },
  stage_started: { label: "Plan stage started", icon: "▶" },
  implementation_started: { label: "Implementation started", icon: "▶" },
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
  if (link.kind !== "file") return link.path || link.run_id || "Open";
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
        link.plan_id ? `data-plan-id="${esc(link.plan_id)}"` : "",
        link.stage_id ? `data-stage-id="${esc(link.stage_id)}"` : "",
        link.run_id ? `data-run-id="${esc(link.run_id)}"` : "",
        link.line_start != null ? `data-line-start="${Number(link.line_start)}"` : "",
        link.line_end != null ? `data-line-end="${Number(link.line_end)}"` : "",
      ].filter(Boolean).join(" ");
      return `<button type="button" class="thread-reference" ${attributes}>${esc(linkLocation(link))}</button>`;
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
      <div class="thread-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(message.body || "")}</div>
      ${linksHtml(message.links)}
    </div>
  </article>`;
}

function eventHtml(event, agentLabel = "Agent") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  const label = meta.label.replace(/^Agent\b/, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.event !== "done" && event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}${linksHtml(event.links)}</div>
  </div>`;
}

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
// overriding ids/placeholder — caller-scoped ids let two thread surfaces (the
// plan review and the run diff) each mount a composer on one page without
// colliding.
function composerHtml(composer) {
  if (!composer) return "";
  const { inputId, sendId, hintId, placeholder } = {
    ...PLAN_COMPOSER_DEFAULTS,
    ...(composer === true ? {} : composer),
  };
  return `<div class="thread-composer">
    <textarea id="${esc(inputId)}" rows="2" placeholder="${esc(placeholder)}"></textarea>
    <div class="thread-composer-actions"><span class="hint" id="${esc(hintId)}"></span><button class="btn primary mini" id="${esc(sendId)}">Send</button></div>
  </div>`;
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
  return `<section class="review-thread">
    <div class="thread-title"><span class="thread-title-text">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</span>${statusChipHtml(options.status)}</div>
    <div class="thread-items thread-timeline">${renderedItems.length
      ? renderedItems.join("")
      : '<div class="thread-empty">No conversation yet.</div>'}</div>
    <div class="thread-revision-view" hidden></div>
    ${threadActionsHtml(options.actionsId)}
    ${composerHtml(options.composer)}
  </section>`;
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

export function wireThreadLinks(root, openLink) {
  if (!root) return;
  root.querySelectorAll(".thread-reference").forEach((button) => {
    button.onclick = () => {
      const link = { kind: button.dataset.kind };
      if (button.dataset.path) link.path = button.dataset.path;
      if (button.dataset.planId) link.plan_id = button.dataset.planId;
      if (button.dataset.stageId) link.stage_id = button.dataset.stageId;
      if (button.dataset.runId) link.run_id = button.dataset.runId;
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
/// `onSubmit(body)` does the transport and resolves when the post has landed.
export function wireThreadComposer(root, { ids, onSubmit, readDraft, writeDraft, onError, afterSubmit }) {
  if (!root) return;
  const input = root.querySelector(`#${ids.input}`);
  const send = root.querySelector(`#${ids.send}`);
  const hint = ids.hint ? root.querySelector(`#${ids.hint}`) : null;
  if (!input || !send) return;

  input.value = readDraft();
  input.oninput = () => {
    writeDraft(input.value);
    if (hint) hint.textContent = "";
  };

  const submit = async () => {
    // A send is already in flight: the keyboard path has no disabled gate.
    if (send.disabled) return;
    const body = input.value.trim();
    if (!body) {
      if (hint) hint.textContent = "Type a message first.";
      input.focus();
      return;
    }
    send.disabled = true;
    send.textContent = "sending…";
    try {
      const result = await onSubmit(body);
      writeDraft("");
      input.value = "";
      send.disabled = false;
      send.textContent = "Send";
      if (afterSubmit) afterSubmit(result);
    } catch (error) {
      // The text stays put: a failed send must never cost the user their words.
      send.disabled = false;
      send.textContent = "Send";
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
