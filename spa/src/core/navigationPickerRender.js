import { esc } from "./text.js";
import { unreadBadgeHtml } from "./toolbarRender.js";
import { destinationIdentity } from "./navigationPickerModel.js";
import { kindGlyphHtml } from "./environmentIdentity.js";
import { dimDeviceHtml } from "./inbox.js";

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
  if (picker.environmentGrouped) return environmentPickerEntries(picker);
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

function environmentDestinationEntry(destination, group, shortcut = false) {
  const label = destination.name;
  const folders = destination.addedFolders ? ` +${destination.addedFolders} folders` : "";
  const archived = destination.archived ? ' <span class="dim">Archived</span>' : "";
  return {
    key: `${shortcut ? "visit" : "destination"}:${destinationIdentity(destination)}`,
    groupKey: shortcut ? "visits" : group.key,
    html: `<a class="mi${destination.current ? " current" : ""}" data-destination-kind="${destination.kind}" data-destination="${esc(destination.key)}"
      data-${destination.kind}="${esc(destination.key)}"${shortcut ? " data-visit" : ""} href="${esc(destination.hash)}"${destination.current ? ' aria-current="page"' : ""}>
      <span class="mi-line identity-line">${kindGlyphHtml(destination.kind)}<span class="mt">${esc(label)}</span><span class="dim">${esc(folders)}</span>${archived}${unreadBadgeHtml(destination.unreadCount, label)}</span>
      ${dimDeviceHtml(group.deviceLabel)}${destination.kind === "workspace" ? `<span class="md">${esc(destination.projectName)}</span>` : ""}</a>`,
  };
}

function environmentPickerEntries(picker) {
  const rows = [];
  const groupsByProject = new Map(picker.groups.flatMap((group) => group.destinations.map((row) => [row.projectKey, group])));
  if (picker.lastVisited.length) {
    rows.push({ key: "recent", groupKey: "visits", html: '<div class="tb-group">Recent</div>' });
    rows.push(...picker.lastVisited.map((row) => environmentDestinationEntry(row, groupsByProject.get(row.projectKey), true)));
  }
  if (picker.legacyLastVisited.length) {
    rows.push({ key: "last-visited", groupKey: "legacy-visits", html: '<div class="tb-group">Last visited</div>' });
    rows.push(...picker.legacyLastVisited.map(row => ({ ...destinationEntry(row, groupsByProject.get(row.projectKey), true), groupKey: "legacy-visits" })));
  }
  for (const group of picker.groups) rows.push(...environmentGroupEntries(group));
  if (!picker.groups.length) rows.push({ key: "none", html: '<div class="tb-none dim">No matching projects or workspaces</div>' });
  return rows;
}

function environmentGroupEntries(group) {
  if (group.kind !== "environment") return globalPickerEntries({ groups: [group], lastVisited: [] });
  return [{ key: `environment:${group.key}`, groupKey: group.key,
    html: `<div class="tb-group tb-environment-group identity-line" data-environment-group="${esc(group.key)}">${kindGlyphHtml("environment")}<span>${esc(group.name)}</span>${dimDeviceHtml(group.deviceLabel)}</div>` },
  ...group.destinations.map((row) => environmentDestinationEntry(row, group))];
}
