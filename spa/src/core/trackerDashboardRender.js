// The project tracker at a glance. Each section owns a keyed list of links;
// cache announcements repaint the entries without replacing stable rows.

import { KEYED_LIST_ATTRIBUTE, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { esc } from "./text.js";

const SECTIONS = [
  { id: "needsYou", title: "Needs you", empty: "Nothing needs your look right now." },
  { id: "inProgress", title: "In progress", empty: "No agent is working on an issue." },
  { id: "doneToday", title: "Done", empty: "Nothing moved to Done in the last 24 hours." },
];
export const DEFAULT_DASHBOARD_TAB = SECTIONS[0].id;
export const dashboardTabIds = SECTIONS.map((section) => section.id);

const secondaryText = {
  inProgress: (entry) => [entry.agentName, entry.activity].filter(Boolean).join(" · "),
  needsYou: (entry) => (entry.reasonLabels || []).join(" · "),
  doneToday: (entry) => entry.sha ? `Commit ${entry.sha.slice(0, 12)}` : "Moved to Done",
};

const dashboardRowHtml = (entry, context, section) => {
  const issue = entry.issue;
  return `<li class="issue-dashboard-row" data-issue="${esc(issue.id)}">
    <a href="${esc(context.href(issue))}" class="issue-dashboard-link">
      <span class="issue-dashboard-number">#${esc(String(issue.number ?? ""))}</span>
      <span class="issue-dashboard-title">${esc(issue.title)}</span>
      <span class="issue-dashboard-detail">${esc(secondaryText[section](entry))}</span>
    </a>
  </li>`;
};

/** One selected section, from cached records only. The Dashboard's links are
 *  the same issue routes as List and Board, including inside a workspace. */
export function paintIssueDashboard(body, sections, context) {
  const selected = SECTIONS.find((section) => section.id === context.dashboardTab) || SECTIONS[0];
  const entries = sections[selected.id] || [];
  const tabs = SECTIONS.map((section) => `<button type="button" class="issue-dashboard-tab" role="tab"
    id="issue-dashboard-tab-${section.id}" data-dashboard-tab="${section.id}"
    aria-controls="issue-dashboard-panel" aria-selected="${section.id === selected.id}"
    tabindex="${section.id === selected.id ? 0 : -1}"><span>${section.title}</span><span class="issue-dashboard-count">${(sections[section.id] || []).length}</span></button>`).join("");
  const frame = `<div class="issue-dashboard">
    <div class="issue-dashboard-tabs" role="tablist" aria-label="Issue dashboard sections">${tabs}</div>
    <section class="issue-dashboard-section" id="issue-dashboard-panel" role="tabpanel"
      data-dashboard-section="${selected.id}" aria-labelledby="issue-dashboard-tab-${selected.id}">
      <ul class="issue-dashboard-list" ${KEYED_LIST_ATTRIBUTE}></ul>
      ${entries.length ? "" : `<p class="issue-dashboard-empty">${selected.empty}</p>`}
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
  patchList(body.querySelector(".issue-dashboard-list"), entries, {
    keyOf: (entry) => entry.issue.id,
    render: (entry) => dashboardRowHtml(entry, context, selected.id),
  });
}
