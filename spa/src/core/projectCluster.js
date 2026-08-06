// The tab bar's right cluster: the project-wide entries every project surface
// carries, wherever the user is standing (the primary checkout, an external
// worktree, a run, an issue). EVERY project-scoped pane lives here as an icon
// tab — a tab in every respect, selectable and painted into the surface's own
// #tabbody — while the surface's own tabs (conversation, changes, files, agent,
// terminals) keep the row's left side. The ⋯ menu is the entry point for Archive
// (another tab state) and the project settings sheet.
//
// One definition and one body-mounting entry point live here; a surface only
// spreads projectClusterShellOptions() into its mountTabShell call and asks
// mountProjectClusterTab() first when a tab is selected.

import { ICON_CIRCLE_DOT, ICON_INBOX } from "./icons.js";
import { mountProjectInbox } from "../views/project.js";
import { mountArchiveTab } from "../views/archive.js";
import { mountIssuesTab } from "../views/issues.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { openNewIssue as openNewIssueSheet } from "../sheets/newIssue.js";

/** Icon tabs pinned to the row's right. The icon is the cell; the label is the
 *  tooltip and the accessible name. Issues takes the circle-dot every issue
 *  tracker uses, because a cell with no text has one job: be recognised. */
export const PROJECT_CLUSTER_TABS = [
  { id: "inbox", icon: ICON_INBOX, label: "Inbox" },
  { id: "issues", icon: ICON_CIRCLE_DOT, label: "Issues" },
];

/** The ⋯ menu's actions. "archive" is a tab state the surface selects; "settings"
 *  opens a sheet and selects nothing. */
export const PROJECT_CLUSTER_MENU = [
  { id: "archive", label: "Archive", description: "Retired issues and worktrees" },
  { id: "settings", label: "Project settings", description: "Name, path, base branch, remote" },
];

/** Every tab this module owns, and the heading its pane wears. */
const CLUSTER_TAB_TITLE = { inbox: "Inbox", issues: "Issues", archive: "Archive" };

/** True for a tab whose body this module owns, on any surface. */
export const isProjectClusterTab = (tabId) => Object.hasOwn(CLUSTER_TAB_TITLE, tabId);

/** The pane's own name, in the app's page-heading shape. The row shows an icon
 *  and nothing else (Archive not even that), so the body is where the user reads
 *  which pane they are standing in. Written once, here, rather than by each
 *  mounted view — which is also what keeps a pane from titling itself twice.
 *  Returns the element the pane mounts into. */
function paneUnderHeading(host, tabId) {
  host.innerHTML = `<div class="cluster-pane">
    <div class="board-head"><div><h1>${CLUSTER_TAB_TITLE[tabId]}</h1></div></div>
    <div class="cluster-body"></div>
  </div>`;
  return host.querySelector(".cluster-body");
}

/** Mount a cluster tab's body into `host`, or return null when the tab belongs to
 *  the surface itself — or when the surface has not learned its project yet (a
 *  run entered by URL learns it from its first run.get, and remounts). The
 *  returned controller owns its own poll, like every other mounted pane. */
export function mountProjectClusterTab(host, tabId, { projectId, callRpc, navigate, openNewIssue = openNewIssueSheet }) {
  if (!projectId || !isProjectClusterTab(tabId)) return null;
  const body = paneUnderHeading(host, tabId);
  if (tabId === "inbox") return mountProjectInbox(body, { projectId, callRpc });
  if (tabId === "issues") {
    // Filing an issue is the pane's own verb, so the sheet it opens is filed
    // against the project the pane is showing — never the route's.
    return mountIssuesTab(body, { projectId, callRpc, navigate, onNewIssue: () => openNewIssue({ projectId }) });
  }
  return mountArchiveTab(body, { projectId, callRpc, navigate });
}

/** The mountTabShell options that put the cluster on a surface's row. Empty
 *  until the surface knows its project — a run learns that from its first
 *  run.get, and a cluster without a project id has nothing to open. */
export function projectClusterShellOptions({ projectId, selectTab, openSettings = openProjectSettings }) {
  if (!projectId) return {};
  return {
    rightTabs: PROJECT_CLUSTER_TABS,
    menu: PROJECT_CLUSTER_MENU,
    onMenuPick: (action) => {
      if (action === "settings") {
        openSettings(projectId);
        return;
      }
      selectTab(action);
    },
  };
}
