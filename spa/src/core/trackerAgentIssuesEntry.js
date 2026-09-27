// What an agent is carrying on the issue board, as one of its SURFACES.
//
// Not a block in the conversation any more (#34). The maintainer, on the #14
// entry as it rolled: "Right now the in review issues on an agent are just
// noise taking up space on the chat. I had envisioned the issues activity to be
// the same UX as agents/workflows/tasks/shells. So it's only visible when the
// user wants it to be and there's a clear pattern for done work."
//
// So this file no longer draws anything. It supplies rows, and the surfaces
// layer draws them behind a pill like every other kind — which also gets the
// fold for finished work for free, rather than this file inventing a second
// one beside it.
//
// The rows come off the project's cached issue list. It asks the bridge for
// nothing: the list is already on disk (core/trackerCache.js), the ordered
// pass keeps it warm, and an `issues` push makes core/cacheSync.js rewrite
// that record — so subscribing to the RECORD gives a live surface with no
// read, no second subscription and no full pass.
//
// It supplies from that cache alone and never asks whether the bridge is
// connected or what its greeting said, so a cold or offline start shows the
// pill off the cached list (#104 review). Where there is no list — a bridge that
// has never carried issues writes none — nothing is supplied and no pill
// appears. Not an empty one: a pill that is always there and opens on nothing
// is one a reader learns to skip.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { agentIssueEntries } from "./trackerAgentIssues.js";

/**
 * Mount the supplier.
 *
 * `onChanged` is called whenever the answer to `entriesFor` would differ — a
 * pushed list, or the first read landing — and the rail re-syncs its surfaces
 * off it. Nothing here knows which agent is in focus: the rail asks per agent
 * at paint time, because the focus moves far more often than the list does.
 */
export function mountAgentIssues({ deviceId, projectId, onChanged } = {}) {
  const state = { issues: [], disposed: false };
  const named = Boolean(deviceId) && Boolean(projectId);

  async function reread() {
    if (!named) return;
    const record = await readIssuesRecord(deviceId, projectId);
    if (state.disposed) return;
    state.issues = record?.issues || [];
    onChanged?.();
  }

  const unsubscribe = named && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

  void reread();

  return {
    /** The project's issues as this rail has read them. What a `#42` written
     *  in a message is resolved against (core/referenceTargets.js): the list
     *  is already here, and reading it twice would be two answers to one
     *  question (#63). */
    issues: () => state.issues,

    /** This agent's issues as surface entries, or none at all — which is what
     *  keeps the pill away from an agent holding and tracking nothing. */
    entriesFor(agentId) {
      if (!named || !agentId) return null;
      const entries = agentIssueEntries(state.issues, agentId);
      return entries.length ? entries : null;
    },
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
    },
  };
}
