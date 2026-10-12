import { esc } from "./text.js";
import { unreadBadgeHtml } from "./toolbarRender.js";
import { destinationIdentity } from "./navigationPickerModel.js";

function destinationLabel(destination, group, shortcut) {
  const name = destination.kind === "project" ? "Project workspace" : destination.name;
  return shortcut ? `${group.name} / ${name}` : name;
}

function destinationEntry(destination, group, shortcut = false) {
  const label = destinationLabel(destination, group, shortcut);
  const context = `${group.name} · ${group.deviceName}`;
  const status = destination.current ? "Current" : destination.statusText || "";
  return {
    key: `${shortcut ? "visit" : "destination"}:${destinationIdentity(destination)}`,
    groupKey: shortcut ? "visits" : group.key,
    html: `<a class="mi${destination.current ? " current" : ""}" data-destination-kind="${destination.kind}" data-destination="${esc(destination.key)}"
      data-${destination.kind}="${esc(destination.key)}"${shortcut ? ' data-visit' : ""}
      href="${esc(destination.hash)}"${destination.current ? ' aria-current="page"' : ""}>
      <span class="mi-line"><span class="mt">${esc(label)}</span>${unreadBadgeHtml(destination.unreadCount, label)}</span>
      <span class="md">${esc([context, status].filter(Boolean).join(" · "))}</span></a>`,
  };
}

export function globalPickerEntries(picker) {
  const groups = new Map(picker.groups.map((group) => [group.key, group]));
  const rows = [];
  if (picker.lastVisited.length) {
    rows.push({ key: "last-visited", groupKey: "visits", html: '<div class="tb-group">Last visited</div>' });
    rows.push(...picker.lastVisited.map((row) => destinationEntry(row, groups.get(row.projectKey), true)));
  }
  for (const group of picker.groups) {
    rows.push({ key: `group:${group.key}`, groupKey: group.key, html: `<div class="tb-group tb-project-group" data-project-group="${esc(group.key)}">
      <span>${esc(group.name)} · ${esc(group.deviceName)}</span>${unreadBadgeHtml(group.unreadCount, group.name)}</div>` });
    rows.push(...group.destinations.map((row) => destinationEntry(row, group)));
  }
  if (!picker.groups.length) rows.push({ key: "none", html: '<div class="tb-none dim">No matching projects or workspaces</div>' });
  return rows;
}
