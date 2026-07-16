// Task presentation helpers shared by the board, notifications, and task views.

import { $ } from "../dom.js";
import { App } from "../app.js";

export const STATE_LABEL = {
  plan_review: "READY TO REVIEW",
  review: "READY TO REVIEW",
  planning: "PLANNING",
  building: "BUILDING",
  created: "CREATED",
  merged: "MERGED",
  abandoned: "ABANDONED",
  archived: "ARCHIVED",
  blocked: "BLOCKED",
  failed: "FAILED",
};

export function chipClass(state) {
  if (state === "blocked" || state === "failed") return "warn";
  if (state === "merged") return "done";
  if (state === "plan_review" || state === "review") return "attn";
  return "work";
}

export function payloadFor(task) {
  if (task.summary) return task.summary;
  if (task.state === "planning") return "drafting plan…";
  if (task.state === "building") return "coding agent working…";
  return "";
}

/** Terminal display states (the board's DONE bucket; never running/needs-you). */
export const TERMINAL_STATES = new Set(["merged", "abandoned", "archived"]);

/** Tasks that are waiting on the user (the board's "NEEDS YOU" bucket). */
export function attnTasks(tasks) {
  return tasks.filter((t) => t.needs_attention && !TERMINAL_STATES.has(t.state));
}

export function setBadge(tasks) {
  const unread = attnTasks(tasks).filter((t) => !App.readIds.has(t.task_id)).length;
  const badge = $("#notif");
  if (!badge) return;
  badge.style.display = unread ? "inline-block" : "none";
  badge.textContent = unread;
}
