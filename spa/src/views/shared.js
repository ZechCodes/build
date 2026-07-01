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

/** Tasks that are waiting on the user (the board's "NEEDS YOU" bucket). */
export function attnTasks(tasks) {
  return tasks.filter((t) => t.needs_attention && t.state !== "merged" && t.state !== "abandoned");
}

export function setBadge(tasks) {
  const unread = attnTasks(tasks).filter((t) => !App.readIds.has(t.task_id)).length;
  const badge = $("#notif");
  badge.style.display = unread ? "inline-block" : "none";
  badge.textContent = unread;
}
