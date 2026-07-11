// One shared task-feed poller for surfaces that live on every page (the
// sidebar, the nav badge) — views keep their own detail polls.

import { App } from "../app.js";

const subscribers = new Set();
let timer = null;
let last = null;

/** Subscribe to feed snapshots ({tasks, externalWorktrees, projects,
 *  primaryChanges}); the current snapshot (if any) is delivered immediately.
 *  Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (last) fn(last);
  return () => subscribers.delete(fn);
}

async function tick() {
  try {
    const [list, projectList] = await Promise.all([
      App.call("task.list"),
      App.call("project.list"),
    ]);
    last = {
      tasks: list.tasks || [],
      externalWorktrees: list.external_worktrees || [],
      primaryChanges: list.primary_changes || [],
      projects: projectList.projects || [],
    };
    subscribers.forEach((fn) => fn(last));
  } catch {
    /* offline / transient — the next tick retries */
  }
}

export function startFeed(intervalMs = 2000) {
  stopFeed();
  tick();
  timer = setInterval(tick, intervalMs);
}

export function stopFeed() {
  if (timer) clearInterval(timer);
  timer = null;
}

/** Force an immediate refresh (after adding a project, adopting, …). */
export function refreshFeed() {
  return tick();
}
