// The workspace's issues: the icon beside the settings cog, and what it opens.
//
// The icon carries a count of the open issues this workspace's agents hold —
// open meaning not finished, because a badge is a call to look and finished
// work is not one. Pressing it opens an overlay of those issues grouped by
// agent, drawn with the same cards and the same folds the conversation entry
// uses (core/trackerAgentIssuesRender.js), so an issue looks like itself
// wherever it is read.
//
// Everything comes off the project's cached list. The badge and the overlay
// both listen to that record, so an `issues` push moves the count on the bar
// and the rows under an open overlay without a read: core/cacheSync.js
// rewrites the record and core/localCache.js announces it.
//
// Nothing is drawn at all on a bridge whose greeting does not advertise the
// issues kind — no icon, not an icon reading zero.

import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { subscribeCache } from "./localCache.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { carriesIssuesPush } from "./trackerPush.js";
import { columnsOf } from "./trackerModel.js";
import { workspaceIssueGroups, workspaceOpenIssueCount } from "./trackerWorkspaceIssues.js";
import { agentIssuesHtml } from "./trackerAgentIssuesRender.js";

export const WORKSPACE_ISSUES_SELECTOR = "[data-workspace-issues]";
const OVERLAY_BODY_CLASS = "workspace-issues-body";

/** What the overlay says when the workspace's agents hold nothing. The one
 *  place an empty state IS drawn: the reader pressed a button to get here and
 *  is owed an answer, unlike an entry that simply appears. */
const EMPTY_HTML = `<p class="empty">No issues assigned in this workspace.</p>`;

const sectionHtml = (section, context) => `<section class="workspace-issues-agent" data-issues-agent="${esc(section.agentId)}">
    <h3 class="workspace-issues-agent-head">${esc(section.label)}</h3>
    ${agentIssuesHtml(section.groups, context)}
  </section>`;

/** The overlay's body: one section per agent holding something, or the empty
 *  line. Pure, so the DOM tests read it without a modal. */
export function workspaceIssuesBodyHtml(sections, context = {}) {
  if (!sections.length) return EMPTY_HTML;
  return sections.map((section) => sectionHtml(section, context)).join("");
}

const overlayHtml = (name) =>
  modalDialogHtml(`<h3>Issues in ${esc(name || "this workspace")}</h3><div class="${OVERLAY_BODY_CLASS}"></div>`, {
    className: "modal-surface modal-workspace-issues",
  });

/**
 * Mount the icon's badge and its press.
 *
 * `button` is the element the toolbar drew; `agents` is read at press and at
 * every repaint rather than captured, because a workspace gains and loses
 * agents while the bar stands there.
 */
export function mountWorkspaceIssues(button, { deviceId, projectId, workspaceName, agents }) {
  const carries = Boolean(deviceId) && Boolean(projectId) && carriesIssuesPush(deviceId);
  const state = { issues: [], columns: [], disposed: false, overlay: null };
  const place = { projectId: projectId || null, deviceId: deviceId || null };

  const paintBadge = () => {
    if (state.disposed || !button) return;
    const count = carries ? workspaceOpenIssueCount(state.issues, agents()) : 0;
    const badge = button.querySelector(".tb-issues-count");
    if (badge) badge.textContent = count ? String(count) : "";
    // The icon stays whether or not anything is open — it is how the view is
    // reached — but it says nothing when there is nothing waiting.
    button.hidden = !carries;
    button.classList.toggle("has-issues", count > 0);
    button.setAttribute(
      "title",
      count ? `${count} open issue${count === 1 ? "" : "s"} in this workspace` : "Issues in this workspace",
    );
  };

  const paintOverlay = () => {
    const body = state.overlay?.body?.querySelector(`.${OVERLAY_BODY_CLASS}`);
    if (!body) return;
    body.innerHTML = workspaceIssuesBodyHtml(workspaceIssueGroups(state.issues, agents()), {
      columns: state.columns,
      place,
    });
  };

  async function reread() {
    if (!carries) return;
    const record = await readIssuesRecord(deviceId, projectId);
    if (state.disposed) return;
    state.issues = record?.issues || [];
    state.columns = columnsOf(record?.columns);
    paintBadge();
    paintOverlay();
  }

  const open = () => {
    if (state.overlay) return state.overlay;
    const modal = openModal({
      dialogHtml: overlayHtml(workspaceName()),
      onClose: () => {
        state.overlay = null;
      },
    });
    state.overlay = modal;
    paintOverlay();
    return modal;
  };

  if (button) button.onclick = open;
  const unsubscribe = carries && subscribeCache(issuesAddress(deviceId, projectId), () => void reread());

  paintBadge();
  void reread();

  return {
    /** The bar repainted or the workspace's agents moved: the count is read
     *  again off what is already in hand, with nothing asked of the bridge. */
    refresh: paintBadge,
    open,
    dispose() {
      state.disposed = true;
      if (unsubscribe) unsubscribe();
      state.overlay?.close?.();
      state.overlay = null;
    },
  };
}
