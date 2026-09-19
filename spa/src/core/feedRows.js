// One machine's rows, read off the shared feed snapshot without subscribing.
//
// The feed replays its last snapshot to every new subscriber, so a subscribe-
// and-drop is how a surface reads it synchronously while it mounts. What a
// surface wants is never the merge: every machine mints a `proj-1`, the merge
// holds all of them at once, and a surface is about the one machine its link
// named. Both halves of that read live here so no surface writes them again.

import { subscribeFeed } from "./taskFeed.js";
import { deviceFeedView } from "./deviceContexts.js";

/** One machine's slice of the feed as it stands right now, or null while the
 *  feed has nothing to replay. */
export function deviceFeedNow(deviceId) {
  let snapshot = null;
  const unsubscribe = subscribeFeed((feed) => {
    snapshot = deviceFeedView(feed, deviceId);
  });
  unsubscribe();
  return snapshot;
}

/** That machine's row for one branch of one of its projects, or null when the
 *  slice carries none. */
export const branchRowIn = (snapshot, projectId, branch) =>
  (snapshot?.items || []).find(
    (item) => item.kind === "branch" && item.project_id === projectId && item.branch === branch,
  ) || null;

/** The live run body the board carries beside its rows — the goal, the state,
 *  the base the diff is measured against, the triage pass. A row that names no
 *  run has none, and neither has a slice the board has not filled. */
export const runBodyIn = (snapshot, runId) =>
  (runId && (snapshot?.runs || []).find((run) => run.run_id === runId)) || null;

/** A checkout on this branch the board's own list leaves out: a worktree
 *  nobody has adopted earns no inbox row, and a link to one still has to open.
 *  Shaped as the branch row the surfaces are written against. */
export const worktreeRowIn = (snapshot, projectId, branch) => {
  const held = (snapshot?.externalWorktrees || []).find(
    (checkout) => checkout.project_id === projectId && checkout.branch === branch,
  );
  return held ? { ...held, kind: "branch", run: null } : null;
};
