// One shared board.list-feed poller for surfaces that live on every page (the
// sidebar, the nav badge) — views keep their own detail polls. The plan/run
// split means the feed carries both collections; consumers read `plans` and
// `runs` separately.

import { App } from "../app.js";
import { pruneReadIds, persistReadIds } from "./readState.js";

const subscribers = new Set();
let timer = null;
let last = null;

/** Subscribe to feed snapshots ({plans, runs, externalWorktrees, projects,
 *  primaryChanges}); the current snapshot (if any) is delivered immediately.
 *  Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (last) fn(last);
  return () => subscribers.delete(fn);
}

async function tick() {
  try {
    const [board, projectList] = await Promise.all([
      App.call("board.list"),
      App.call("project.list"),
    ]);
    last = {
      plans: board.plans || [],
      runs: board.runs || [],
      externalWorktrees: board.external_worktrees || [],
      primaryChanges: board.primary_changes || [],
      projects: projectList.projects || [],
    };
    // Prune read-state against EVERY live id (not just needs-attention ones) so
    // the persisted set can't grow forever, while an entity that re-enters
    // needs-attention stays read.
    const liveIds = [...last.runs.map((r) => r.run_id), ...last.plans.map((p) => p.plan_id)];
    App.readIds = pruneReadIds(App.readIds, liveIds);
    persistReadIds(App.readIds, localStorage);
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
