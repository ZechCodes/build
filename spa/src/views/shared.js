// Presentation helpers shared by the board, notifications, project, and the
// run/plan views. The plan/run split gives each entity its own state vocabulary
// (see the wire contract), so labels, chip palettes, terminal sets, and payload
// copy come in matching run/plan variants.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { RUN_TERMINAL_STATES, PLAN_TERMINAL_STATES } from "../core/board.js";

// The terminal-state sets are the board model's source of truth (core/board.js);
// re-exported here so the run/plan presentation helpers and their importers keep
// a single import point.
export { RUN_TERMINAL_STATES, PLAN_TERMINAL_STATES };

// ---- Runs (worktree-scoped; "Tasks" in the UI) --------------------------------

export const RUN_STATE_LABEL = {
  created: "CREATED",
  building: "BUILDING",
  stage_gate: "STAGE GATE",
  review: "READY TO REVIEW",
  blocked: "BLOCKED",
  failed: "FAILED",
  idle_unreported: "IDLE",
  interrupted: "INTERRUPTED",
  merged: "MERGED",
  abandoned: "ABANDONED",
  archived: "ARCHIVED",
};

export function runChipClass(state) {
  if (state === "blocked" || state === "failed") return "warn";
  if (state === "merged") return "done";
  if (state === "review") return "attn";
  return "work";
}

export function runPayloadFor(run) {
  if (run.summary) return run.summary;
  if (run.state === "building") return "coding agent working…";
  return "";
}

/** Runs waiting on the user (the board's "NEEDS YOU" bucket). */
export function attnRuns(runs) {
  return (runs || []).filter((r) => r.needs_attention && !RUN_TERMINAL_STATES.has(r.state));
}

// ---- Plans (project-scoped) ---------------------------------------------------

export const PLAN_STATE_LABEL = {
  created: "CREATED",
  drafting: "PLANNING",
  plan_review: "READY TO REVIEW",
  approved: "APPROVED",
  blocked: "BLOCKED",
  failed: "FAILED",
  idle_unreported: "IDLE",
  interrupted: "INTERRUPTED",
  abandoned: "ABANDONED",
};

export function planChipClass(state) {
  if (state === "blocked" || state === "failed") return "warn";
  if (state === "approved") return "done";
  if (state === "plan_review") return "attn";
  return "work";
}

export function planPayloadFor(plan) {
  if (plan.summary) return plan.summary;
  if (plan.state === "drafting" || plan.state === "created") return "drafting plan…";
  return "";
}

/** Plans waiting on the user. */
export function attnPlans(plans) {
  return (plans || []).filter((p) => p.needs_attention && !PLAN_TERMINAL_STATES.has(p.state));
}

// ---- The attention badge (runs + plans) --------------------------------------

/** The nav badge counts every run *and* plan that needs the user and is not yet
 *  read. Runs are keyed by run_id, plans by plan_id. */
export function setBadge(runs, plans = []) {
  const unread =
    attnRuns(runs).filter((r) => !App.readIds.has(r.run_id)).length +
    attnPlans(plans).filter((p) => !App.readIds.has(p.plan_id)).length;
  const badge = $("#notif");
  if (!badge) return;
  badge.style.display = unread ? "inline-block" : "none";
  badge.textContent = unread;
}
