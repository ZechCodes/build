// One shared board.list-feed poller for surfaces that live on every page (the
// sidebar, the nav badge) — views keep their own detail polls. The plan/run
// split means the feed carries both collections; consumers read `plans` and
// `runs` separately.

import { App } from "../app.js";

const subscribers = new Set();
let timer = null;
let last = null;

/** Subscribe to feed snapshots ({items, plans, runs, externalWorktrees,
 *  projects, primaryChanges}); the current snapshot (if any) is delivered
 *  immediately. Returns unsubscribe. */
export function subscribeFeed(fn) {
  subscribers.add(fn);
  if (last) fn(last);
  return () => subscribers.delete(fn);
}

/** The run that owns a project's primary checkout, or null while nobody has
 *  adopted it. The bridge stamps the owner onto the feed's primary-changes
 *  entry, which makes this a READ: a surface can bind to an existing owner
 *  without minting one. A terminal run has let go, so it is reported as null. */
export function primaryRunIdFor(feed, projectId) {
  const entry = ((feed && feed.primaryChanges) || []).find((e) => e.project_id === projectId);
  return (entry && entry.run_id) || null;
}

async function tick() {
  try {
    const [board, projectList] = await Promise.all([
      App.call("board.list"),
      App.call("project.list"),
    ]);
    last = {
      // The redesigned feed: one row per work item (branch or issue). The
      // legacy collections below still ship, and still feed what has not moved
      // over yet.
      items: board.items || [],
      plans: board.plans || [],
      runs: board.runs || [],
      externalWorktrees: board.external_worktrees || [],
      primaryChanges: board.primary_changes || [],
      // The wire names a project by `project_id`; consumers of the snapshot
      // (the toolbar's scope and menu) read `id`. Bridge the key here, in the
      // one place the wire is read.
      projects: (projectList.projects || []).map((project) => ({
        ...project,
        id: project.project_id || project.id,
      })),
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
