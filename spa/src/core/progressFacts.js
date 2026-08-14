// One-line progress facts for a working run's card: live diffstat, how
// recently anything moved, and how long it has sat in its current state.
// Pure — the card templates feed it a board.list run and Date.now(). Fields
// an older bridge doesn't send simply drop their part.

import { humanAge } from "./text.js";
import { RUN_TERMINAL_STATES } from "./board.js";

// The lowercase human word for "<state> for 12m". Gate/review get phrasing
// that reads as a position, not a command.
const RUN_STATE_WORD = {
  created: "created",
  building: "building",
  stage_gate: "at stage gate",
  review: "in review",
  blocked: "blocked",
  failed: "failed",
  idle_unreported: "idle",
  interrupted: "interrupted",
};

/** Bare duration (no "ago"): "<1m", "12m", "3h", "2d". */
function humanDuration(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "<1m";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** Seconds between nowMs and an RFC 3339 stamp, or null when unparsable. */
function secondsSince(stamp, nowMs) {
  const parsed = Date.parse(stamp || "");
  if (Number.isNaN(parsed)) return null;
  return (nowMs - parsed) / 1000;
}

/** The same lowercase word, over the one state vocabulary a board.list row can
 *  speak: run states, issue states, and the bare checkout's `idle` (which says
 *  nothing worth a line). */
const ITEM_STATE_WORD = {
  ...RUN_STATE_WORD,
  drafting: "drafting",
  plan_review: "in review",
  approved: "ready to implement",
  merged: "merged",
  abandoned: "abandoned",
  archived: "archived",
};

/** How long the turn in flight has been running. The bridge sends both the
 *  start and its own resolution of it: prefer the stamp, so a client that keeps
 *  the row on screen ticks, and fall back to the number when it cannot parse. */
function workingSeconds(workingTime, nowMs) {
  if (!workingTime) return null;
  const ticking = secondsSince(workingTime.since, nowMs);
  if (ticking !== null) return ticking;
  return Number.isFinite(workingTime.seconds) ? workingTime.seconds : null;
}

/**
 * The same one-line facts for a board.list `items[]` row — the shape the inbox
 * reads. "3 files · +42 −7 · working 15m" while an agent has it; otherwise what
 * state it sits in and when it was last picked up. "" when nothing is known.
 */
export function itemProgressFacts(item, nowMs) {
  const parts = [];
  const stat = item && item.stat;
  if (stat && stat.files_changed > 0) {
    parts.push(`${stat.files_changed} file${stat.files_changed === 1 ? "" : "s"}`);
    parts.push(`+${stat.insertions || 0} −${stat.deletions || 0}`);
  }
  const working = workingSeconds(item && item.working_time, nowMs);
  if (working !== null) {
    parts.push(`working ${humanDuration(working)}`);
    return parts.join(" · ");
  }
  const word = ITEM_STATE_WORD[item && item.state];
  if (word) parts.push(word);
  const resumed = secondsSince(item && item.resume_at, nowMs);
  if (resumed !== null) parts.push(`picked up ${humanAge(resumed)}`);
  return parts.join(" · ");
}

/**
 * "3 files · +42 −7 · active 5m ago · building for 12m" for a non-terminal
 * run; "" for terminal runs or when nothing is known.
 */
export function runProgressFacts(run, nowMs) {
  if (!run || RUN_TERMINAL_STATES.has(run.state)) return "";
  const parts = [];
  const stat = run.stat;
  if (stat && stat.files_changed > 0) {
    parts.push(`${stat.files_changed} file${stat.files_changed === 1 ? "" : "s"}`);
    parts.push(`+${stat.insertions || 0} −${stat.deletions || 0}`);
  }
  const sinceActive = secondsSince(run.updated_at, nowMs);
  if (sinceActive !== null) parts.push(`active ${humanAge(sinceActive)}`);
  const inState = secondsSince(run.state_changed_at, nowMs);
  if (inState !== null) {
    const word = RUN_STATE_WORD[run.state] || run.state;
    parts.push(`${word} for ${humanDuration(inState)}`);
  }
  return parts.join(" · ");
}
