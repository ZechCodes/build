import { esc } from "./text.js";

const EVENT_LABELS = {
  session_started: "Agent session started",
  session_ended: "Agent session ended",
  run_started: "Run started",
  run_failed: "Run failed",
  idle_unreported: "Agent went idle without reporting",
  done: "Agent reported done",
  revision_created: "Revision created",
};

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
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  return `<article class="thread-message ${user ? "user" : "agent"}">
    <div class="thread-message-head"><strong>${user ? "You" : "Agent"}</strong>${status}</div>
    ${anchorLabel(message.anchor)}
    <div class="thread-body">${esc(message.body || "").replace(/\n/g, "<br>")}</div>
  </article>`;
}

function eventHtml(event) {
  const label = EVENT_LABELS[event.event] || String(event.event || "event").replaceAll("_", " ");
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.summary ? esc(event.summary) : "";
  return `<div class="thread-event"><span>${esc(label)}</span>${detail ? `<span>${detail}</span>` : ""}</div>`;
}

function completionHtml(report) {
  if (!report) return "";
  const groups = [
    ["Critical files", report.critical_files],
    ["Risks", report.risk_notes],
    ["Decisions", report.decisions],
    ["Skipped", report.skips],
  ].filter(([, values]) => values && values.length);
  return `<article class="thread-message agent thread-completion">
    <div class="thread-message-head"><strong>Agent</strong></div>
    <div class="thread-body"><strong>Completion report</strong>${groups
    .map(([label, values]) => `<div><strong>${label}</strong><ul>${values.map((value) => `<li>${esc(value)}</li>`).join("")}</ul></div>`)
    .join("")}</div>
  </article>`;
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
  const completion = completionHtml(thread && thread.last_completion);
  const itemCount = items.length + (completion ? 1 : 0);
  return `<section class="review-thread">
    <div class="thread-title">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</div>
    <div class="thread-items">${items.length || completion
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
