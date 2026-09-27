// The workspace's Issues face on its rail (core/directoryRail.js): the count of
// what its agents hold, and whether the face is drawn at all.
//
// It wears the unread of the watched issues this workspace's agents hold
// (#104): "The issues tab should carry the watched issue unread count … for
// workspaces it is only unreads on issues assigned to an agent in that
// workspace." How many of those issues are open — not finished — is said in
// its tooltip, which is where the count it wore before #104 went.
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
// It paints from that cache alone and never asks whether the bridge is
// connected or what its greeting said: on a cold or offline start the face and
// its count come straight off the cached list (#104 review). Only a route that
// names no project has no face to draw.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { workspaceIssuesTitle, workspaceIssuesUnread, workspaceOpenIssueCount } from "./trackerWorkspaceIssues.js";

/**
 * Mount the face's badge.
 *
 * `button` is the cell the rail drew; `agents` is read at every repaint rather
 * than captured, because a workspace gains and loses agents while the rail
 * stands there.
 */
export function mountWorkspaceIssues(button, { deviceId, projectId, agents }) {
  const named = Boolean(deviceId) && Boolean(projectId);
  const state = { issues: [], disposed: false };

  const paintBadge = () => {
    if (state.disposed || !button) return;
    const unread = named ? workspaceIssuesUnread(state.issues, agents()) : 0;
    const open = named ? workspaceOpenIssueCount(state.issues, agents()) : 0;
    const badge = button.querySelector(".dirtab-count");
    if (badge) badge.textContent = unread ? String(unread) : "";
    // The entry stays whether or not anything is waiting — it is how the view
    // is reached — but its bubble says nothing when nothing is unread.
    button.hidden = !named;
    button.classList.toggle("has-issues", open > 0);
    button.setAttribute("title", workspaceIssuesTitle(unread, open));
  };

  async function reread() {
    if (!named) return;
    const record = await readIssuesRecord(deviceId, projectId);
    if (state.disposed) return;
    state.issues = record?.issues || [];
    paintBadge();
  }

  const unsubscribe = named && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

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
