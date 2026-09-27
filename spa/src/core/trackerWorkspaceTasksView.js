// The workspace's Tasks face on its rail (core/directoryRail.js): the count of
// what its agents hold, and whether the face is drawn at all.
//
// It wears the unread of the watched tasks this workspace's agents hold
// (#104): "The tasks tab should carry the watched task unread count … for
// workspaces it is only unreads on tasks assigned to an agent in that
// workspace." How many of those tasks are open — not finished — is said in
// its tooltip, which is where the count it wore before #104 went.
//
// It used to open an overlay of those tasks (#16), then became a word in the
// bar beside the directory tabs (#47). It is an icon on the rail now (#174):
// "Tasks moved to the rail as an icon along with the settings. Making the left
// rail the workspace navigation." The press is the rail's — a face like
// Changes and Files, going to the workspace's Tasks TAB (#29,
// core/workspaceTasksTab.js) — so this module knows the count and nothing
// about where it leads.
//
// The count comes off the project's cached list and listens to that record, so
// an `tasks` push moves it on the rail with nothing asked of the bridge:
// core/cacheSync.js rewrites the record and core/localCache.js announces it.
//
// It paints from that cache alone and never asks whether the bridge is
// connected or what its greeting said: on a cold or offline start the face and
// its count come straight off the cached list (#104 review). Only a route that
// names no project has no face to draw.

import { subscribeCache } from "./localCache.js";
import { tasksAddress, readTasksRecord } from "./trackerCache.js";
import { workspaceTasksTitle, workspaceTasksUnread, workspaceOpenTaskCount } from "./trackerWorkspaceTasks.js";

/**
 * Mount the face's badge.
 *
 * `button` is the cell the rail drew; `agents` is read at every repaint rather
 * than captured, because a workspace gains and loses agents while the rail
 * stands there.
 */
export function mountWorkspaceTasks(button, { deviceId, projectId, agents }) {
  const named = Boolean(deviceId) && Boolean(projectId);
  const state = { tasks: [], disposed: false };

  const paintBadge = () => {
    if (state.disposed || !button) return;
    const unread = named ? workspaceTasksUnread(state.tasks, agents()) : 0;
    const open = named ? workspaceOpenTaskCount(state.tasks, agents()) : 0;
    const badge = button.querySelector(".dirtab-count");
    if (badge) badge.textContent = unread ? String(unread) : "";
    // The entry stays whether or not anything is waiting — it is how the view
    // is reached — but its bubble says nothing when nothing is unread.
    button.hidden = !named;
    button.classList.toggle("has-tasks", open > 0);
    button.setAttribute("title", workspaceTasksTitle(unread, open));
  };

  async function reread() {
    if (!named) return;
    const record = await readTasksRecord(deviceId, projectId);
    if (state.disposed) return;
    state.tasks = record?.tasks || [];
    paintBadge();
  }

  const unsubscribe = named && subscribeCache(tasksAddress(deviceId, projectId), () => void reread());

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
