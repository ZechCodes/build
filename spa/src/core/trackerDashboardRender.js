// The project tracker at a glance. Each section owns a keyed list of links;
// cache announcements repaint the entries without replacing stable rows.

import { KEYED_LIST_ATTRIBUTE, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { esc } from "./text.js";

const SECTIONS = [
  { id: "inProgress", title: "In progress", empty: "No agent is working on an issue." },
  { id: "needsYou", title: "Needs you", empty: "Nothing needs your look right now." },
  { id: "doneToday", title: "Done today", empty: "Nothing moved to Done in the last 24 hours." },
];

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

/** Three sections, from cached records only. The Dashboard's links are the
 *  same issue routes as List and Board, including inside a workspace. */
export function paintIssueDashboard(body, sections, context) {
  const frame = `<div class="issue-dashboard" role="group" aria-label="Issue dashboard">${SECTIONS.map((section) => {
    const entries = sections[section.id] || [];
    return `<section class="issue-dashboard-section" data-dashboard-section="${section.id}" aria-label="${section.title}">
      <header class="issue-dashboard-head"><h2>${section.title}</h2><span>${entries.length}</span></header>
      <ul class="issue-dashboard-list" ${KEYED_LIST_ATTRIBUTE}></ul>
      ${entries.length ? "" : `<p class="issue-dashboard-empty">${section.empty}</p>`}
    </section>`;
  }).join("")}</div>`;
  patchInnerHtml(body, frame);
  for (const section of SECTIONS) {
    const panel = body.querySelector(`[data-dashboard-section="${section.id}"]`);
    patchList(panel.querySelector(".issue-dashboard-list"), sections[section.id] || [], {
      keyOf: (entry) => entry.issue.id,
      render: (entry) => dashboardRowHtml(entry, context, section.id),
    });
  }
}
