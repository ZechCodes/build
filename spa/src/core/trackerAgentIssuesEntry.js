// The issues entry in a conversation's activity area, mounted.
//
// It reads the project's cached issue list and draws the four groups for the
// agent in focus. It asks the bridge for nothing: the list is already on disk
// (core/trackerCache.js), the ordered pass keeps it warm, and an `issues` push
// makes core/cacheSync.js rewrite that record — so subscribing to the RECORD
// repaints this entry on the push with no read, no second subscription and no
// full pass. It is the same way the console and the changes review stay live.
//
// Nothing is drawn on a bridge whose greeting does not advertise the issues
// kind. Not an empty box: an entry that is always there and usually empty is
// one a reader learns to skip past, and on such a bridge there is nothing to
// put in it anyway.

import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { carriesIssuesPush } from "./trackerPush.js";
import { columnsOf } from "./trackerModel.js";
import { agentIssueGroups } from "./trackerAgentIssues.js";
import { agentIssuesHtml } from "./trackerAgentIssuesRender.js";

/**
 * Mount the entry into `host`.
 *
 * `set(agentId)` says whose issues to draw — the rail calls it whenever the
 * agent in focus changes, and with null when there is none. `dispose()` stops
 * listening.
 */
export function mountAgentIssues(host, { deviceId, projectId } = {}) {
  const state = { agentId: null, issues: [], columns: [], disposed: false };
  const place = { projectId: projectId || null, deviceId: deviceId || null };
  // Asked once, at mount: a bridge does not gain the kind without a new
  // greeting, and a new greeting remounts the rail.
  const carries = Boolean(deviceId) && Boolean(projectId) && carriesIssuesPush(deviceId);

  const paint = () => {
    if (state.disposed || !host) return;
    const html = carries
      ? agentIssuesHtml(agentIssueGroups(state.issues, state.agentId), { columns: state.columns, place })
      : "";
    if (host.innerHTML !== html) host.innerHTML = html;
    host.hidden = !html;
  };

  async function reread() {
    if (!carries) return;
    const record = await readIssuesRecord(deviceId, projectId);
    if (state.disposed) return;
    state.issues = record?.issues || [];
    state.columns = columnsOf(record?.columns);
    paint();
  }

  const unsubscribe =
    carries && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

  paint();
  void reread();

  return {
    set(agentId) {
      const next = agentId || null;
      if (state.agentId === next) return;
      state.agentId = next;
      paint();
    },
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
    },
  };
}
