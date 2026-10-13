// Linked blocker numbers use escaped cached titles. They stand outside the
// task's own link so keyboard navigation never nests anchors.
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { taskIsBlocked, taskWaitingText, unresolvedTaskBlockers } from "./trackerTaskReadiness.js";

export function taskBlockerLinksHtml(blockers, { deviceId, projectId }) {
  const links = blockers.map((blocker) => {
    const href = hashFromRoute({ name: "trackerTask", deviceId, projectId, taskId: blocker.task_id });
    return `<a class="task-blocker-link" href="${esc(href)}" title="${esc(blocker.title || "")}">#${esc(String(blocker.number ?? ""))}</a>`;
  });
  return `Waiting on ${links.join(", ")}`;
}

export function taskWaitingHtml(task, context) {
  if (!taskIsBlocked(task)) return "";
  const blockers = unresolvedTaskBlockers(task);
  const words = blockers.length ? taskBlockerLinksHtml(blockers, context) : esc(taskWaitingText(task));
  return `<span class="task-waiting">${words}</span>`;
}

/** The inbox keeps its plain facts for sorting/attention consumers, replacing
 * only the waiting phrase with the same linked numbers when painting it. */
export function taskInboxFactsHtml(entry, facts) {
  const escaped = esc(facts);
  if (!entry.taskBlockers?.length) return escaped;
  const words = `Waiting on ${entry.taskBlockers.map((blocker) => `#${blocker.number}`).join(", ")}`;
  return escaped.replace(esc(words), taskBlockerLinksHtml(entry.taskBlockers, entry));
}
