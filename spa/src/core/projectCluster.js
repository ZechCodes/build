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

import { mountProjectInbox } from "../views/project.js";
import { mountArchiveTab } from "../views/archive.js";
import { mountIssuesTab } from "../views/issues.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { openNewIssue as openNewIssueSheet } from "../sheets/newIssue.js";

/** Icon tabs pinned to the row's right. The glyph is the cell; the label is the
 *  tooltip and the accessible name. */
export const PROJECT_CLUSTER_TABS = [
  { id: "inbox", glyph: "▤", label: "Inbox" },
  { id: "issues", glyph: "◎", label: "Issues" },
];

/** The ⋯ menu's actions. "archive" is a tab state the surface selects; "settings"
 *  opens a sheet and selects nothing. */
export const PROJECT_CLUSTER_MENU = [
  { id: "archive", label: "Archive", description: "Retired issues and worktrees" },
  { id: "settings", label: "Project settings", description: "Name, path, base branch, remote" },
];

const CLUSTER_TAB_IDS = new Set(["inbox", "issues", "archive"]);

/** True for a tab whose body this module owns, on any surface. */
export const isProjectClusterTab = (tabId) => CLUSTER_TAB_IDS.has(tabId);

/** Mount a cluster tab's body into `host`, or return null when the tab belongs to
 *  the surface itself — or when the surface has not learned its project yet (a
 *  run entered by URL learns it from its first run.get, and remounts). The
 *  returned controller owns its own poll, like every other mounted pane. */
export function mountProjectClusterTab(host, tabId, { projectId, callRpc, navigate, openNewIssue = openNewIssueSheet }) {
  if (!projectId) return null;
  if (tabId === "inbox") return mountProjectInbox(host, { projectId, callRpc });
  if (tabId === "issues") {
    // Filing an issue is the pane's own verb, so the sheet it opens is filed
    // against the project the pane is showing — never the route's.
    return mountIssuesTab(host, { projectId, callRpc, navigate, onNewIssue: () => openNewIssue({ projectId }) });
  }
  if (tabId === "archive") return mountArchiveTab(host, { projectId, callRpc, navigate });
  return null;
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
