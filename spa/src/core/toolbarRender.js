// Cache supplies names; URL supplies ancestry. Every segment has its own target.
import { workspaceLockHtml } from "./workspaceLock.js";
import { esc } from "./text.js";
import { toolbarBreadcrumbs } from "./toolbarBreadcrumbs.js";
import { ICON_CHEVRON_LEFT } from "./icons.js";
import { kindGlyphHtml, projectIdentityText } from "./environmentIdentity.js";
import { patchElement } from "./domPatch.js";

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
  const projectLink = projectCrumbHtml(project, shown);
  const items = [itemHtml(projectLink, "tb-project-item", false)];
  if (workspace) items.push(itemHtml(selectorHtml(workspace.label,
    `Switch workspace in ${shown.project}; current: ${workspace.label}`, workspace.current), "tb-workspace-item"));
  if (tasks) items.push(itemHtml(linkHtml(tasks, "tb-tasks", "data-tasks-parent"), "tb-tasks-item"));
  if (leaf) items.push(itemHtml(linkHtml(leaf, `tb-leaf${shown.kind === "branch" ? " mono" : ""}`), "tb-leaf-item"));
  return `<nav class="tb-breadcrumbs${tasks ? " tb-task-ancestry" : ""}" aria-label="Breadcrumb"><ol>${items.join("")}</ol></nav>`;
}

function projectCrumbHtml(project, shown) {
  if (shown.environmentIdentity) return environmentCrumbHtml(project, shown.environmentIdentity);
  const qualifier = shown.deviceTag ? ` · ${shown.deviceTag}` : "";
  return linkHtml({ ...project, label: project.label + qualifier }, "tb-project",
    `data-project-return aria-label="${esc(`Open project workspace ${project.label}${qualifier}`)}"`);
}

/** Capability and environment changes only patch the existing project link.
 * Its label, ancestry, triggers and other mounted controls keep their nodes. */
export function patchToolbarProjectIdentity(host, shown) {
  const link = host.querySelector("[data-project-return]");
  if (!link) return;
  const template = host.ownerDocument.createElement("template");
  template.innerHTML = projectCrumbHtml(toolbarBreadcrumbs(shown).project, shown);
  const next = template.content.firstElementChild;
  for (const attribute of ["class", "title", "aria-label"]) {
    if (link.getAttribute(attribute) !== next.getAttribute(attribute)) link.setAttribute(attribute, next.getAttribute(attribute));
  }
  const name = link.querySelector(".tb-name");
  patchElement(name, next.querySelector(".tb-name"));
  for (const selector of [".identity-kind", ".tb-environment", ".tb-environment-sep"]) {
    patchCrumbPrefix(link, next, selector, name);
  }
}

function patchCrumbPrefix(link, next, selector, name) {
  const live = link.querySelector(selector);
  const wanted = next.querySelector(selector);
  if (!wanted) { live?.remove(); return; }
  if (live) patchElement(live, wanted);
  else link.insertBefore(wanted, name);
}

function environmentCrumbHtml(project, identity) {
  const environment = identity.showEnvironment
    ? `<span class="tb-environment dim">${esc(identity.environmentName)}</span><span class="tb-environment-sep dim"> › </span>` : "";
  const label = projectIdentityText(identity);
  return `<a class="tb-project tb-environment-project" href="${esc(project.hash)}" data-project-return
    aria-label="${esc(`Open project workspace ${label}`)}" title="${esc(label)}"${project.current ? ' aria-current="page"' : ""}>
    ${kindGlyphHtml("project")}${environment}<span class="tb-name">${esc(project.label)}</span></a>`;
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
