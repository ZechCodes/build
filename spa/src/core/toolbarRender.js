// The view-area toolbar's markup: where you are standing, and the directories
// of the workspace you are standing in.
//
// Pure — names come from repos, agents and the user, so every one of them is
// escaped. core/toolbar.js holds the state and does the wiring; this file only
// says what the bar looks like for one identity.

import { esc } from "./text.js";
import { ICON_CHEVRON_LEFT, ICON_SETTINGS } from "./icons.js";

/** One of the bar's popup triggers. The workspace switcher, the project
 *  selector and the collapsed directory menu are the same control wearing a
 *  different name, so the button is written once. */
const selectorHtml = ({ select, className = `tb-${select}`, name, ariaLabel = "" }) =>
  `<button class="tb-sel ${className}" data-select="${select}" type="button" aria-haspopup="menu" aria-expanded="false"${ariaLabel ? ` aria-label="${esc(ariaLabel)}"` : ""}>
     <span class="tb-name">${esc(name)}</span><span class="tb-caret disclosure-caret" aria-hidden="true">▾</span>
   </button>`;

const projectSelectorHtml = (project) => selectorHtml({ select: "project", name: project || "Projects" });

/** A branch or an issue hangs off the project selector and says its own name
 *  after it; a branch name is a wire string, so it wears the mono face. */
const legacyIdentityHtml = (mono) => ({ project, label }) =>
  `${projectSelectorHtml(project)}
     <span class="tb-sep">/</span><span class="tb-legacy-item"><span class="tb-name${mono ? " mono" : ""}">${esc(label)}</span></span>`;

/** The way back out of a workspace: to the project it was cut from — the
 *  project's own page, where the inbox's project name goes too. Named for the
 *  project, so a reader hearing it knows where it goes. */
const backToProjectHtml = (project) => {
  const label = project ? `Back to ${project}` : "Back to the project";
  return `<button class="iconbtn tb-back" data-project-back type="button" aria-label="${esc(label)}" title="${esc(label)}">${ICON_CHEVRON_LEFT}</button>`;
};

/** What the bar stands you in, one writer per kind of route. A workspace names
 *  itself through its own switcher, with the way back to its project before it;
 *  a route that is no work item at all is the project selector and nothing
 *  else. */
const IDENTITIES = {
  workspace: ({ project, label }) => `${backToProjectHtml(project)}${selectorHtml({ select: "workspace", name: label })}`,
  branch: legacyIdentityHtml(true),
  issue: legacyIdentityHtml(false),
};

const identityHtml = (shown) => (IDENTITIES[shown.kind] || (() => projectSelectorHtml(shown.project)))(shown);

/** The directory tabs of the workspace you are standing in, with the menu the
 *  same tabs collapse into once the bar runs out of room. */
function directoryTabsHtml(directories) {
  if (!directories.length) return "";
  const tabs = directories
    .map(
      (directory) =>
        `<button class="tb-directory${directory.current ? " current" : ""}" data-directory="${esc(directory.sourceId)}" type="button" role="tab" aria-selected="${directory.current ? "true" : "false"}">${esc(directory.label)}</button>`,
    )
    .join("");
  const chosen = directories.find((directory) => directory.current) || directories[0];
  return `<div class="tb-directories" role="tablist" aria-label="Workspace directories">${tabs}</div>
       ${selectorHtml({
         select: "directory",
         className: "tb-directory-menu",
         name: chosen.label,
         ariaLabel: "Choose workspace directory",
       })}`;
}

/** The project's two pages as tabs after its name, the same tab a workspace's
 *  directories are — and never collapsed into a menu: two short words fit a
 *  phone, and on a phone they are the way back to the list from an issue. */
function projectTabsHtml(projectTabs) {
  if (!projectTabs.length) return "";
  const tabs = projectTabs
    .map(
      (tab) =>
        `<button class="tb-directory tb-project-tab${tab.current ? " current" : ""}" data-project-tab="${esc(tab.id)}" type="button" role="tab" aria-selected="${tab.current ? "true" : "false"}">${esc(tab.label)}</button>`,
    )
    .join("");
  return `<div class="tb-project-tabs" role="tablist" aria-label="Project pages">${tabs}</div>`;
}

/** What this workspace's agents are carrying on the issue board, beside the
 *  cog. Drawn hidden and shown by whoever mounts it
 *  (core/trackerWorkspaceIssuesView.js): a bridge that does not carry issues
 *  gets nothing at all rather than an entry reading zero. The count lives in
 *  its own span so the push can move it without repainting the bar — the verb
 *  slot can be holding an open menu.
 *
 *  A word rather than an icon, and beside the directory tabs rather than by the
 *  cog (#47): it belongs to the workspace the way the name and the tabs do, and
 *  the cog's corner is for settings. It is not inside `.tb-directories`, which
 *  collapses into a menu on a phone — the issues are reachable at every width,
 *  which is what putting them in the rail had bought. */
const issuesTabHtml = (kind, { current = false } = {}) =>
  kind === "workspace"
    ? `<button class="tb-directory tb-issues${current ? " current" : ""}" data-workspace-issues type="button" aria-label="Issues in this workspace" title="Issues in this workspace"${current ? ' aria-current="page"' : ""} hidden>Issues<span class="badge tb-issues-count"></span></button>`
    : "";

/** The cog at the far right, opposite the switcher at the far left: what this
 *  workspace is called, what its agents start on, and the one way to delete it.
 *  Only a route standing IN a workspace has one to settle, so only a workspace
 *  identity gets the button. */
const settingsButtonHtml = (kind) =>
  kind === "workspace"
    ? `<button class="iconbtn tb-settings" data-workspace-settings type="button" aria-label="Workspace settings" title="Workspace settings">${ICON_SETTINGS}</button>`
    : "";

/** Pure: the toolbar's markup for one identity. Names come from repos, agents
 *  and the user, so every one of them is escaped. */
export function toolbarHtml({ project, kind, label, directories = [], projectTabs = [], workspaceIssues = {} }) {
  return `<div class="toolbar">
    ${identityHtml({ project, kind, label })}
    ${projectTabsHtml(projectTabs)}
    ${directoryTabsHtml(directories)}
    ${issuesTabHtml(kind, workspaceIssues)}
    <div class="tb-right">${settingsButtonHtml(kind)}<span class="tb-verb" id="tb-verb"></span></div>
  </div>`;
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
export function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}
