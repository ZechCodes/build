// The workspace's Issues face on its rail (core/directoryRail.js): the count of
// what its agents hold, and whether the face is drawn at all.
//
// It wears a count of the open issues this workspace's agents hold — open
// meaning not finished, because a badge is a call to look and finished work is
// not one.
//
// It used to open an overlay of those issues (#16), then became a word in the
// bar beside the directory tabs (#47). It is an icon on the rail now (#174):
// "Issues moved to the rail as an icon along with the settings. Making the left
// rail the workspace navigation." The press is the rail's — a face like
// Changes and Files, going to the workspace's Issues TAB (#29,
// core/workspaceIssuesTab.js) — so this module knows the count and nothing
// about where it leads.
//
// The count comes off the project's cached list and listens to that record, so
// an `issues` push moves it on the rail with nothing asked of the bridge:
// core/cacheSync.js rewrites the record and core/localCache.js announces it.
//
// Nothing is drawn at all on a bridge whose greeting does not advertise the
// issues kind — no face, not a face reading zero.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { carriesIssuesPush } from "./trackerPush.js";
import { workspaceOpenIssueCount } from "./trackerWorkspaceIssues.js";

/**
 * Mount the face's badge.
 *
 * `button` is the cell the rail drew; `agents` is read at every repaint rather
 * than captured, because a workspace gains and loses agents while the rail
 * stands there.
 */
export function mountWorkspaceIssues(button, { deviceId, projectId, agents }) {
  const carries = Boolean(deviceId) && Boolean(projectId) && carriesIssuesPush(deviceId);
  const state = { issues: [], disposed: false };

  const paintBadge = () => {
    if (state.disposed || !button) return;
    const count = carries ? workspaceOpenIssueCount(state.issues, agents()) : 0;
    const badge = button.querySelector(".dirtab-count");
    if (badge) badge.textContent = count ? String(count) : "";
    // The entry stays whether or not anything is open — it is how the view is
    // reached — but its bubble says nothing when there is nothing waiting.
    button.hidden = !carries;
    button.classList.toggle("has-issues", count > 0);
    button.setAttribute(
      "title",
      count ? `${count} open issue${count === 1 ? "" : "s"} in this workspace` : "Issues in this workspace",
    );
  };

  async function reread() {
    if (!carries) return;
    const record = await readIssuesRecord(deviceId, projectId);
    if (state.disposed) return;
    state.issues = record?.issues || [];
    paintBadge();
  }

  const unsubscribe = carries && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

  paintBadge();
  void reread();

  return {
    /** The workspace's agents moved: the count is read again off what is
     *  already in hand, with nothing asked of the bridge. */
    refresh: paintBadge,
    /** The rail repainted and drew a new cell: say the count on it. */
    retarget(next) {
      button = next;
      paintBadge();
    },
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
    },
  };
}
