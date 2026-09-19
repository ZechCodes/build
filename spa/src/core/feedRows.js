// One machine's rows, read off the shared feed snapshot without subscribing.
//
// The feed replays its last snapshot to every new subscriber, so a subscribe-
// and-drop is how a surface reads it synchronously while it mounts. What a
// surface wants is never the merge: every machine mints a `proj-1`, the merge
// holds all of them at once, and a surface is about the one machine its link
// named. Both halves of that read live here so no surface writes them again.

import { subscribeFeed } from "./taskFeed.js";
import { subscribeCache } from "./localCache.js";
import { deviceFeedHeld, deviceFeedView } from "./deviceContexts.js";

/** The record kinds a machine's board is written in: the whole list, which a
 *  pass and a board removal write, and one row's own record, which is what a
 *  `state` push writes and the only thing that moves within a pass.
 *
 *  For the pages that hold no records of their own — the capture decision and
 *  the archive, neither of which was in the cache-first brief — this is the
 *  whole of what says "read again". Hearing only the list would leave them on
 *  the last pass's answer while the row they are about has already moved. */
const BOARD_WRITES = new Set(["feed", "row"]);

/** Hear every write that can move a machine's board. Returns unsubscribe. */
export const subscribeBoardWrites = (read) =>
  subscribeCache({}, (address) => {
    if (BOARD_WRITES.has(address.kind)) read();
  });

/** One machine's slice of the feed as it stands right now, or null while the
 *  feed holds nothing FOR THAT MACHINE — it has nothing to replay at all, or
 *  what it replays is other machines'. Null is "not loaded", never "listed
 *  nothing": the merge is the whole account's, and a machine whose first pass
 *  has not landed is simply absent from it. */
export function deviceFeedNow(deviceId) {
  let snapshot = null;
  const unsubscribe = subscribeFeed((feed) => {
    snapshot = deviceFeedHeld(feed, deviceId) ? deviceFeedView(feed, deviceId) : null;
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
