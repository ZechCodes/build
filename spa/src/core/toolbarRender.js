// The view-area toolbar's markup: where you are standing, said as one
// breadcrumb.
//
// Pure — names come from repos, agents and the user, so every one of them is
// escaped. core/toolbar.js holds the state and does the wiring; this file only
// says what the bar looks like for one identity.

import { esc } from "./text.js";

/** One of the bar's popup triggers. The workspace picker and the project
 *  selector are the same control wearing a different name, so the button is
 *  written once; `lead` is markup said inside it before the name. */
const selectorHtml = ({ select, className = `tb-${select}`, name, lead = "" }) =>
  `<button class="tb-sel ${className}" data-select="${select}" type="button" aria-haspopup="menu" aria-expanded="false">
     ${lead}<span class="tb-name">${esc(name)}</span><span class="tb-caret disclosure-caret" aria-hidden="true">▾</span>
   </button>`;

const projectSelectorHtml = (project) => selectorHtml({ select: "project", name: project || "Projects" });

/** A branch or an issue hangs off the project selector and says its own name
 *  after it; a branch name is a wire string, so it wears the mono face. */
const legacyIdentityHtml = (mono) => ({ project, label }) =>
  `${projectSelectorHtml(project)}
     <span class="tb-sep">/</span><span class="tb-legacy-item"><span class="tb-name${mono ? " mono" : ""}">${esc(label)}</span></span>`;

/** A workspace is one picker reading `project / workspace`: one control, one
 *  breadcrumb, the project said quietly before the workspace it holds. Its menu
 *  moves between workspaces and reaches the project's own page. The project half
 *  is only said when there is a name to say. */
const workspacePickerHtml = ({ project, label }) =>
  selectorHtml({
    select: "workspace",
    name: label,
    lead: project ? `<span class="tb-crumb">${esc(project)}</span><span class="tb-sep">/</span>` : "",
  });

/** What the bar stands you in, one writer per kind of route. A workspace names
 *  itself through its picker; a route that is no work item at all is the
 *  project selector and nothing else. */
const IDENTITIES = {
  workspace: workspacePickerHtml,
  branch: legacyIdentityHtml(true),
  issue: legacyIdentityHtml(false),
};

const identityHtml = (shown) => (IDENTITIES[shown.kind] || (() => projectSelectorHtml(shown.project)))(shown);

/** The project's two pages as tabs after its name — never collapsed into a
 *  menu: two short words fit a phone, and on a phone they are the way back to
 *  the list from an issue. */
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

/** Pure: the toolbar's markup for one identity. Names come from repos, agents
 *  and the user, so every one of them is escaped. */
export function toolbarHtml({ project, kind, label, projectTabs = [] }) {
  return `<div class="toolbar">
    ${identityHtml({ project, kind, label })}
    ${projectTabsHtml(projectTabs)}
    <div class="tb-right"><span class="tb-verb" id="tb-verb"></span></div>
  </div>`;
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
export function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}
