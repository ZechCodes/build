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

function messageHtml(message) {
  const user = message.role === "user";
  const completion = message.source === "completion";
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}${completion ? " thread-completion" : ""}">
    <span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>
    <div class="thread-comment-card">
      <div class="thread-message-head"><span><strong>${user ? "You" : "Agent"}</strong> ${completion ? "reported completion" : "commented"} ${timeHtml(message.created_at)}</span>${status}</div>
      ${anchorLabel(message.anchor)}
      <div class="thread-body markdown">${renderMarkdown(message.body || "")}</div>
    </div>
  </article>`;
}

function eventHtml(event) {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(meta.label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}</div>
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

function completionHtml(report) {
  return report ? messageHtml({ role: "agent", source: "completion", body: completionBody(report) }) : "";
}

function composerHtml(enabled) {
  if (!enabled) return "";
  return `<div class="thread-composer">
    <textarea id="planthreadinput" rows="3" placeholder="Send a message to the planning agent…"></textarea>
    <div class="thread-composer-actions"><span class="hint" id="planthreadhint"></span><button class="btn primary" id="planthreadsend">Send</button></div>
  </div>`;
}

export function threadHtml(thread, options = {}) {
  const sourceItems = (thread && thread.items) || [];
  const initialMessage = String(options.initialMessage || "").trim();
  const hasInitialMessage = sourceItems.some(
    (item) => item.type === "message" && item.data?.role === "user" && String(item.data.body || "").trim() === initialMessage,
  );
  const items = initialMessage && !hasInitialMessage
    ? [{ type: "message", data: { role: "user", body: initialMessage, seen_at: "initial" } }, ...sourceItems]
    : sourceItems;
  const hasSequencedCompletion = sourceItems.some((item) => item.type === "message" && item.data?.source === "completion");
  const completion = hasSequencedCompletion ? "" : completionHtml(thread && thread.last_completion);
  const itemCount = items.length + (completion ? 1 : 0);
  return `<section class="review-thread">
    <div class="thread-title">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</div>
    <div class="thread-items thread-timeline">${items.length || completion
      ? items.map((item) => item.type === "message" ? messageHtml(item.data || {}) : eventHtml(item.data || {})).join("") + completion
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
