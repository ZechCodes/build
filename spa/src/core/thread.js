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
  implementation_started: { label: "Implementation started", icon: "▶" },
  committed: { label: "Changes committed", icon: "◆", tone: "success" },
  pushed: { label: "Changes pushed", icon: "↑", tone: "success" },
  merged: { label: "Changes merged", icon: "⌁", tone: "success" },
  abandoned: { label: "Abandoned", icon: "×", tone: "blocked" },
};

function timeHtml(createdAt) {
  if (!createdAt) return "";
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return "";
  const label = date.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
  return `<time datetime="${esc(createdAt)}">on ${esc(label)}</time>`;
}

// Client half of the thread cursor: the detail polls (plan.get / run.get every
// 1.6s) would otherwise re-ship the whole forever-growing conversation over
// E2EE on every tick. The cache holds one entity's accumulated items, tells the
// caller which cursor to send, and folds each delta back into a full thread for
// rendering. A `thread_total` that disagrees with what it holds (bridge
// restart, entity swap, dropped delta) resets it to a full refetch.
export function createThreadCache() {
  let accumulatedItems = [];

  const lastHeldSequence = () =>
    accumulatedItems.length ? accumulatedItems[accumulatedItems.length - 1].data?.sequence || 0 : 0;

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
      const heldSequences = new Set(accumulatedItems.map((item) => item.data?.sequence));
      const merged = [
        ...accumulatedItems,
        ...arrivedItems.filter((item) => !heldSequences.has(item.data?.sequence)),
      ].sort((a, b) => (a.data?.sequence || 0) - (b.data?.sequence || 0));
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

function messageHtml(message, agentLabel = "Agent") {
  const user = message.role === "user";
  const completion = message.source === "completion";
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}${completion ? " thread-completion" : ""}">
    <span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>
    <div class="thread-comment-card">
      <div class="thread-message-head"><span><strong>${user ? "You" : esc(agentLabel)}</strong> ${completion ? "reported completion" : "commented"} ${timeHtml(message.created_at)}</span>${status}</div>
      ${anchorLabel(message.anchor)}
      <div class="thread-body markdown">${renderMarkdown(message.body || "")}</div>
    </div>
  </article>`;
}

function eventHtml(event, agentLabel = "Agent") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  const label = meta.label.replace(/^Agent\b/, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}</div>
  </div>`;
}

function completionBody(report) {
  if (!report) return "";
  const groups = [
    ["Critical files", report.critical_files],
    ["Risks", report.risk_notes],
    ["Decisions", report.decisions],
    ["Skipped", report.skips],
  ].filter(([, values]) => values && values.length);
  return ["**Completion report**", ...groups.map(([label, values]) => `**${label}**\n${values.map((value) => `- ${value}`).join("\n")}`)].join("\n\n");
}

function completionHtml(report, agentLabel) {
  return report ? messageHtml({ role: "agent", source: "completion", body: completionBody(report) }, agentLabel) : "";
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
    <textarea id="${esc(inputId)}" rows="3" placeholder="${esc(placeholder)}"></textarea>
    <div class="thread-composer-actions"><span class="hint" id="${esc(hintId)}"></span><button class="btn primary" id="${esc(sendId)}">Send</button></div>
  </div>`;
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
  const hasSequencedCompletion = sourceItems.some((item) => item.type === "message" && item.data?.source === "completion");
  const completion = hasSequencedCompletion ? "" : completionHtml(thread && thread.last_completion, agentLabel);
  const itemCount = items.length + (completion ? 1 : 0);
  return `<section class="review-thread">
    <div class="thread-title">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</div>
    <div class="thread-items thread-timeline">${items.length || completion
      ? items.map((item) => item.type === "message" ? messageHtml(item.data || {}, agentLabel) : eventHtml(item.data || {}, agentLabel)).join("") + completion
      : '<div class="thread-empty">No conversation yet.</div>'}</div>
    <div class="thread-revision-view" hidden></div>
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
