export const EVENT_META = {
  session_started: { label: "Agent session started", icon: "▶" },
  session_ended: { label: "Agent session ended", icon: "■" },
  run_started: { label: "Run started", icon: "▶" },
  run_failed: { label: "Agent reported failure", icon: "×", tone: "blocked" },
  blocked: { label: "Agent reported a blocker", icon: "!", tone: "blocked" },
  review_blocked: { label: "Review blocked", icon: "!", tone: "blocked" },
  idle_unreported: { label: "Agent went idle without reporting", icon: "…", tone: "blocked" },
  done: { label: "Agent reported done", icon: "✓", tone: "success" },
  revision_created: { label: "Revision created", icon: "↻" },
  approved: { label: "Task ready", icon: "✓", tone: "success" },
  stage_approved: { label: "Stage approved", icon: "✓", tone: "success" },
  stage_started: { label: "Stage implementation started", icon: "▶" },
  implementation_started: { label: "Implementation started", icon: "▶" },
  worktree_reused: { label: "Implementation worktree reused", icon: "↻", tone: "success" },
  worktree_recreated: { label: "Implementation worktree recreated", icon: "↻", tone: "success" },
  worktree_deleted: { label: "Implementation worktree deleted", icon: "×", tone: "blocked" },
  recovery_failed: { label: "Verified recovery failed", icon: "×", tone: "blocked" },
  stage_completed: { label: "Stage completed", icon: "✓", tone: "success" },
  stage_invalidated: { label: "Stage marked incomplete", icon: "!", tone: "blocked" },
  implementation_archived: { label: "Implementation archived", icon: "■" },
  committed: { label: "Changes committed", icon: "◆", tone: "success" },
  pushed: { label: "Changes pushed", icon: "↑", tone: "success" },
  merged: { label: "Changes merged", icon: "⌁", tone: "success" },
  abandoned: { label: "Abandoned", icon: "×", tone: "blocked" },
  // Activity: the agent working, rather than the agent speaking. A harness that
  // reports its own reasoning and tool calls has no terminal for them to scroll
  // past in, so they ride the conversation — and they arrive hundreds to a
  // session, which is why `activity` folds them (see activityHtml). None of
  // them carries a tone: not one of them is asking the reader for anything.
  //
  // Their labels are never printed. An activity row is its content, so the
  // label has exactly two jobs left: what the icon says to a screen reader, and
  // the line a row with no summary of its own falls back to.
  reasoning: { label: "Agent thought", icon: "◌", activity: true },
  // A call and its answer are one row: the call mints it, and the answer
  // completes it in place (see `toolOutcomeHtml`).
  tool_use: { label: "Agent called a tool", icon: "▸", activity: true },
  // A row of its own, still minted for an answer whose call the daemon could
  // not pair — and the kind every conversation recorded before the two became
  // one row is full of. Stored rows render forever.
  tool_result: { label: "Tool answered", icon: "◂", activity: true },
  narration: { label: "Agent narrated", icon: "◦", activity: true },
  // Work the agent left running behind its own turn. The label names the task
  // rather than the harness, because the row says what a task is doing and the
  // provider's name in front of it would carry nothing.
  task_update: { label: "Background task", icon: "⧉", activity: true },
  compaction: { label: "Compaction", icon: "↯" },
};

export const STATUS_LINE_EVENTS = new Set(["session_started", "run_started"]);

/// An event kind's label, spoken in the name of the harness that raised it —
/// "Claude Code called a tool" rather than "Agent called a tool".
///
/// On a lifecycle row this is the row's own text. On an activity row it is not
/// text at all: it is what the icon says to a screen reader, and what a row
/// with no summary of its own falls back to.
export function eventLabel(meta, agentLabel) {
  return meta.label.replace(/^Agent\b/, agentLabel);
}

export function startupEventTitle(event, agentLabel = "Agent") {
  const kind = (event && event.event) || "";
  if (!STATUS_LINE_EVENTS.has(kind)) return "";
  return eventLabel(EVENT_META[kind], agentLabel);
}

export function isStartupEvent(item) {
  if (!item || item.type === "message") return false;
  return STATUS_LINE_EVENTS.has((item.data || {}).event);
}
