// The workspace's Issues entry in the bar: the word, a count, and the way in to
// the tab that holds them.
//
// It wears a count of the open issues this workspace's agents hold — open
// meaning not finished, because a badge is a call to look and finished work is
// not one.
//
// It used to open an overlay of those issues (#16). It no longer does: a modal
// is a thing you must close before you can do anything else, and closing it is
// leaving the issue — which is the opposite of what it was for. Pressing it now
// goes to the workspace's Issues TAB (#29, core/workspaceIssuesTab.js), where
// the whole tracker is, with the workspace's agents still in the rail beside
// it. The entry is a shortcut to a place, not a place of its own.
//
// It was a large circle-dot icon by the settings cog, and is a tab beside the
// directory tabs now (#47): the issues belong to the WORKSPACE, like its name
// and its tabs, and the cog's corner is for settings. Which one the reader is
// standing on is the bar's to mark, not this module's — it knows the count and
// the press, and core/toolbarModel.js knows the route.
//
// The count comes off the project's cached list and listens to that record, so
// an `issues` push moves it on the bar with nothing asked of the bridge:
// core/cacheSync.js rewrites the record and core/localCache.js announces it.
//
// Nothing is drawn at all on a bridge whose greeting does not advertise the
// issues kind — no entry, not an entry reading zero.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { carriesIssuesPush } from "./trackerPush.js";
import { workspaceOpenIssueCount } from "./trackerWorkspaceIssues.js";

export const WORKSPACE_ISSUES_SELECTOR = "[data-workspace-issues]";

/**
 * Mount the icon's badge and its press.
 *
 * `button` is the element the toolbar drew; `agents` is read at press and at
 * every repaint rather than captured, because a workspace gains and loses
 * agents while the bar stands there.
 */
export function mountWorkspaceIssues(button, { deviceId, projectId, workspaceId, agents, open }) {
  const carries = Boolean(deviceId) && Boolean(projectId) && carriesIssuesPush(deviceId);
  const state = { issues: [], disposed: false };

  const paintBadge = () => {
    if (state.disposed || !button) return;
    const count = carries ? workspaceOpenIssueCount(state.issues, agents()) : 0;
    const badge = button.querySelector(".tb-issues-count");
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

  /** The press: go to the tab. `open` is injected so this module never reaches
   *  for the router or the app — the toolbar knows where it is standing. */
  const openTab = () => open?.({ deviceId, projectId, workspaceId });

  if (button) button.onclick = openTab;
  const unsubscribe = carries && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

  paintBadge();
  void reread();

  return {
    /** The bar repainted or the workspace's agents moved: the count is read
     *  again off what is already in hand, with nothing asked of the bridge. */
    refresh: paintBadge,
    open: openTab,
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
    },
  };
}
