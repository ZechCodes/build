// Cache supplies names; URL supplies ancestry. Every segment has its own target.
import { workspaceLockHtml } from "./workspaceLock.js";
import { esc } from "./text.js";
import { toolbarBreadcrumbs } from "./toolbarBreadcrumbs.js";
import { ICON_CHEVRON_LEFT } from "./icons.js";

const selectorHtml = (name, label, current = false) =>
  `<button class="tb-sel tb-workspace" data-select="workspace" type="button" aria-haspopup="dialog" aria-expanded="false"
     aria-label="${esc(label)}" title="${esc(label)}"${current ? ' aria-current="page"' : ""}>
     <span class="tb-name">${esc(name)}</span><span class="tb-caret disclosure-caret" aria-hidden="true">▾</span>
   </button>`;

const switcherHtml = () =>
  `<button class="tb-switch" data-select="project" type="button" aria-haspopup="dialog" aria-expanded="false"
     aria-label="Switch project or workspace" title="Switch project or workspace"><span aria-hidden="true">${ICON_CHEVRON_LEFT}</span></button>`;

const linkHtml = (entry, className, attribute = "") =>
  `<a class="${className}" href="${esc(entry.hash || "#/inbox")}" ${attribute}${entry.current ? ' aria-current="page"' : ""}
     title="${esc(entry.label)}"><span class="tb-name">${esc(entry.label)}</span></a>`;
const itemHtml = (html, className, separated = true) =>
  `<li class="${className}">${separated ? '<span class="tb-sep" aria-hidden="true">/</span>' : ""}${html}</li>`;

function ancestryHtml(shown) {
  const { project, workspace, tasks, leaf } = toolbarBreadcrumbs(shown);
  const qualifier = shown.deviceTag ? ` · ${shown.deviceTag}` : "";
  const projectLink = linkHtml({ ...project, label: project.label + qualifier }, "tb-project",
    `data-project-return aria-label="${esc(`Open project workspace ${project.label}${qualifier}`)}"`);
  const items = [itemHtml(projectLink, "tb-project-item", false)];
  if (workspace) items.push(itemHtml(selectorHtml(workspace.label,
    `Switch workspace in ${shown.project}; current: ${workspace.label}`, workspace.current), "tb-workspace-item"));
  if (tasks) items.push(itemHtml(linkHtml(tasks, "tb-tasks", "data-tasks-parent"), "tb-tasks-item"));
  if (leaf) items.push(itemHtml(linkHtml(leaf, `tb-leaf${shown.kind === "branch" ? " mono" : ""}`), "tb-leaf-item"));
  return `<nav class="tb-breadcrumbs${tasks ? " tb-task-ancestry" : ""}" aria-label="Breadcrumb"><ol>${items.join("")}</ol></nav>`;
}

const surfaceName = (name) => ({ inbox: "Inbox", account: "Settings", device: "Device settings", settings: "Settings" }[name] || "Build");

/** Existing renderer-only fixtures may supply kind without a complete route. */
export function toolbarHtml(shown) {
  const route = shown.route || { name: shown.kind };
  const identity = shown.kind ? ancestryHtml({ ...shown, route })
    : `<span class="tb-surface-title">${surfaceName(route.name)}</span>`;
  const localShortcut = shown.kind && shown.kind !== "workspace"
    ? selectorHtml("Workspaces", `Switch workspace in ${shown.project}`) : "";
  const verb = shown.kind === "workspace" ? "" : '<div class="tb-right"><span class="tb-verb" id="tb-verb"></span></div>';
  return `<div class="toolbar">${switcherHtml()}${identity}${localShortcut}${workspaceLockHtml(shown.workspaceLock)}${verb}</div>`;
}

/** Shared badge vocabulary, quiet when there is nothing unread. */
export function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}
