// The project tracker at a glance. Each section owns a keyed list of links;
// cache announcements repaint the entries without replacing stable rows.

import { KEYED_LIST_ATTRIBUTE, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { esc } from "./text.js";
import { taskBubbleHtml } from "./taskUnread.js";

const SECTIONS = [
  { id: "needsYou", title: "Needs you", empty: "Nothing needs your look right now." },
  { id: "active", title: "Active", empty: "No one holds a task right now." },
  { id: "backlog", title: "Backlog", empty: "No unassigned tasks." },
  { id: "done", title: "Done", empty: "Nothing moved to Done in the last 24 hours." },
];
/** Done on a bridge that knows when the user was last here. */
const DONE_SINCE_LEFT = {
  id: "done",
  title: "Done",
  empty: "Nothing has moved to Done since you left.",
};
const sectionsFor = (context) =>
  context.doneSinceLeft ? SECTIONS.map((section) => (section.id === "done" ? DONE_SINCE_LEFT : section)) : SECTIONS;
export const DEFAULT_DASHBOARD_TAB = SECTIONS[0].id;
export const dashboardTabIds = SECTIONS.map((section) => section.id);

const secondaryText = {
  active: (entry) => entry.working
    ? [entry.agentName, "working now", entry.activity].filter(Boolean).join(" · ")
    : [`With ${entry.holder}`, entry.columnName].filter(Boolean).join(" · "),
  needsYou: (entry) => (entry.reasonLabels || []).join(" · "),
  backlog: (entry) => entry.columnName,
  done: (entry) => entry.sha ? `Commit ${entry.sha.slice(0, 12)}` : "Moved to Done",
};

/** The sections drawn as titled groups, each with the attribute its group
 *  blocks are keyed by. */
const GROUP_ATTRIBUTES = { active: "data-active-group", done: "data-done-group" };

/** A row: the number and title, what the section says of it under them, and
 *  a watched task's unread bubble at the right (#104). */
const dashboardRowHtml = (entry, context, section) => {
  const task = entry.task;
  return `<li class="task-dashboard-row" data-task="${esc(task.id)}">
    <a href="${esc(context.href(task))}" class="task-dashboard-link">
      <span class="task-dashboard-number">#${esc(String(task.number ?? ""))}</span>
      <span class="task-dashboard-title">${esc(task.title)}</span>${taskBubbleHtml(task, context.unreadOf)}
      <span class="task-dashboard-detail">${esc(secondaryText[section](entry))}</span>
    </a>
  </li>`;
};

/** One group of a grouped section (Done's ages, Active's work states): its title
 *  above and outside its own panel, and a keyed list of its rows inside. The
 *  list is patched by `paintGroups`, so the block's markup leaves it empty. */
const groupHtml = (group, attribute) => `<div class="task-dashboard-group" ${attribute}="${esc(group.id)}">
    <h3 class="task-dashboard-group-title">${esc(group.title)}</h3>
    <div class="task-dashboard-group-panel"><ul class="task-dashboard-list" ${KEYED_LIST_ATTRIBUTE}></ul></div>
  </div>`;

/** A grouped section: one block per group, keyed by group id, each holding
 *  its own keyed list, so a row keeps its element while it stays in its group. */
function paintGroups(container, groups, context, section) {
  const attribute = GROUP_ATTRIBUTES[section];
  patchList(container, groups, { keyOf: (group) => group.id, render: (group) => groupHtml(group, attribute) });
  const blocks = [...container.children];
  for (const group of groups) {
    const block = blocks.find((element) => element.getAttribute(attribute) === group.id);
    patchList(block.querySelector(".task-dashboard-list"), group.entries, {
      keyOf: (entry) => entry.task.id,
      render: (entry) => dashboardRowHtml(entry, context, section),
    });
  }
}

/** One selected section, from cached records only. The Dashboard's links are
 *  the same task routes as List and Board, including inside a workspace. */
export function paintTaskDashboard(body, sections, context) {
  const shown = sectionsFor(context);
  const selected = shown.find((section) => section.id === context.dashboardTab) || shown[0];
  const entries = sections[selected.id] || [];
  const groups = GROUP_ATTRIBUTES[selected.id] ? sections[`${selected.id}Groups`] : null;
  const grouped = Array.isArray(groups) && entries.length > 0;
  const tabs = shown.map((section) => `<button type="button" class="task-dashboard-tab" role="tab"
    id="task-dashboard-tab-${section.id}" data-dashboard-tab="${section.id}"
    aria-controls="task-dashboard-panel" aria-selected="${section.id === selected.id}"
    tabindex="${section.id === selected.id ? 0 : -1}"><span>${section.title}</span><span class="task-dashboard-count">${(sections[section.id] || []).length}</span></button>`).join("");
  const frame = `<div class="task-dashboard">
    <div class="task-dashboard-tabs" role="tablist" aria-label="Task dashboard sections">${tabs}</div>
    <section class="task-dashboard-section${grouped ? " is-grouped" : ""}" id="task-dashboard-panel" role="tabpanel"
      data-dashboard-section="${selected.id}" aria-labelledby="task-dashboard-tab-${selected.id}">
      ${grouped ? `<div class="task-dashboard-groups" ${KEYED_LIST_ATTRIBUTE}></div>` : `<ul class="task-dashboard-list" ${KEYED_LIST_ATTRIBUTE}></ul>`}
      ${entries.length ? "" : `<p class="task-dashboard-empty">${selected.empty}</p>`}
    </section>
  </div>`;
  patchInnerHtml(body, frame);
  body.querySelectorAll("[data-dashboard-tab]").forEach((tab) => {
    tab.onclick = () => context.onDashboardTab(tab.dataset.dashboardTab);
    tab.onkeydown = (event) => {
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (!step) return;
      event.preventDefault();
      const index = dashboardTabIds.indexOf(tab.dataset.dashboardTab);
      const next = dashboardTabIds[(index + step + dashboardTabIds.length) % dashboardTabIds.length];
      context.onDashboardTab(next);
      body.querySelector(`[data-dashboard-tab="${next}"]`)?.focus();
    };
  });
  if (grouped) {
    paintGroups(body.querySelector(".task-dashboard-groups"), groups, context, selected.id);
    return;
  }
  patchList(body.querySelector(".task-dashboard-list"), entries, {
    keyOf: (entry) => entry.task.id,
    render: (entry) => dashboardRowHtml(entry, context, selected.id),
  });
}
