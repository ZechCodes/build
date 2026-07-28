// Pure presentation helpers for runs and plans: state labels, chip palettes,
// payload copy, and the attention filters. The plan/run split gives each entity
// its own state vocabulary (see the wire contract), so labels, chip palettes,
// terminal sets, and payload copy come in matching run/plan variants.
//
// This module imports only from core/board.js (the terminal-state sets), keeping
// it free of DOM/app dependencies so it is unit-testable in node and safe to
// import from other core modules (e.g. core/sidebar.js). views/shared.js
// re-exports everything here so existing importers stay unchanged.

import { RUN_TERMINAL_STATES, PLAN_TERMINAL_STATES } from "./board.js";

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
  if (state === "blocked" || state === "failed" || state === "idle_unreported" || state === "interrupted") return "warn";
  if (state === "merged") return "done";
  if (state === "review") return "attn";
  return "work";
}

export function runPayloadFor(run) {
  if (run.summary) return run.summary;
  if (run.state === "building") return "Building — you'll be notified. No need to stay.";
  return "";
}

/** Runs waiting on the user (core/board.js's "NEEDS YOU" bucket). */
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

/** A plan's terse rail/list word — lowercase so a dense list stays scannable;
 *  the full label (PLAN_STATE_LABEL) rides the row's title instead. */
const PLAN_STATE_WORD = {
  plan_review: "review",
  drafting: "drafting",
  created: "drafting",
  approved: "ready",
  blocked: "blocked",
  failed: "failed",
  idle_unreported: "idle",
  interrupted: "interrupted",
  abandoned: "abandoned",
};
export const planStateWord = (state) => PLAN_STATE_WORD[state] || state || "";

export function planChipClass(state) {
  if (state === "blocked" || state === "failed" || state === "idle_unreported" || state === "interrupted") return "warn";
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
