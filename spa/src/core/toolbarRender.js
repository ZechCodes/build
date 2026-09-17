// The view-area toolbar's markup: where you are standing, and the directories
// of the workspace you are standing in.
//
// Pure — names come from repos, agents and the user, so every one of them is
// escaped. core/toolbar.js holds the state and does the wiring; this file only
// says what the bar looks like for one identity.

import { esc } from "./text.js";
import { ICON_SETTINGS } from "./icons.js";

/** One of the bar's popup triggers. The workspace switcher, the project
 *  selector and the collapsed directory menu are the same control wearing a
 *  different name, so the button is written once. */
const selectorHtml = ({ select, className = `tb-${select}`, name, ariaLabel = "" }) =>
  `<button class="tb-sel ${className}" data-select="${select}" type="button" aria-haspopup="menu" aria-expanded="false"${ariaLabel ? ` aria-label="${esc(ariaLabel)}"` : ""}>
     <span class="tb-name">${esc(name)}</span><span class="tb-caret">▾</span>
   </button>`;

const projectSelectorHtml = (project) => selectorHtml({ select: "project", name: project || "Projects" });

/** A branch or an issue hangs off the project selector and says its own name
 *  after it; a branch name is a wire string, so it wears the mono face. */
const legacyIdentityHtml = (mono) => ({ project, label }) =>
  `${projectSelectorHtml(project)}
     <span class="tb-sep">/</span><span class="tb-legacy-item"><span class="tb-name${mono ? " mono" : ""}">${esc(label)}</span></span>`;

/** What the bar stands you in, one writer per kind of route. A workspace names
 *  itself through its own switcher; a route that is no work item at all is the
 *  project selector and nothing else. */
const IDENTITIES = {
  workspace: ({ label }) => selectorHtml({ select: "workspace", name: label }),
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
export function toolbarHtml({ project, kind, label, directories = [] }) {
  return `<div class="toolbar">
    ${identityHtml({ project, kind, label })}
    ${directoryTabsHtml(directories)}
    <div class="tb-right">${settingsButtonHtml(kind)}<span class="tb-verb" id="tb-verb"></span></div>
  </div>`;
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
export function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}
