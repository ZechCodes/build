// The scenes that stand on the project's own pages: the Issues board (act 3),
// the issue with its three agents in the chat overview (act 4), and the
// workflow builder (act 5, a feature the app does not have yet, drawn as a
// third project page in the app's frame).
import { ICON_CHEVRON_DOWN, ICON_EYE, ICON_PLUS } from "../../spa/src/core/icons.js";
import {
  PROJECT_VERBS, agentRail, appShell, harnessMark, overviewPanel, overviewRow, overviewSection, projectToolbar,
} from "./app-shell.js";
import { INBOX_TEAM } from "./workspace-scenes.js";
import { COLUMNS, ISSUES, TEAM } from "./story.js";

const INBOX_BEFORE_TEAM = ["search", "terminals", "pairing"];
const PROJECT_TABS = ["Issues", "Workspaces"];

const caret = ICON_CHEVRON_DOWN.replace('class="lucide', 'class="fmenu-caret lucide');
const filter = (label, set = false) =>
  `<div class="fmenu"><button class="fmenu-press${set ? " is-set" : ""}" type="button"><span class="fmenu-press-label">${label}</span>${caret}</button></div>`;

const viewSwitch = (views, active, action) => `<div class="issue-head">
    <div class="issue-views" role="group">${views.map((view) => `<button class="btn mini issue-view${view === active ? " active" : ""}" type="button">${view}</button>`).join("")}</div>
    <button class="btn mini primary issue-new" type="button">${ICON_PLUS}<span>${action}</span></button>
  </div>`;

const assignee = (name) => name
  ? `<span class="issue-assignee"><span class="issue-avatar is-harness" aria-hidden="true">${harnessMark(name === "Codex" ? "codex" : "claude")}</span>${name}</span>`
  : '<span class="issue-assignee issue-unassigned">Unassigned</span>';

const issueCard = (issue) => `<li class="issue-card" data-status="${issue.status}" data-issue-number="${issue.number}">
    <div class="issue-card-head"><span class="issue-state issue-state-open"></span><span class="issue-number">#${issue.number}</span>${issue.priority ? `<span class="issue-priority issue-priority-${issue.priority}">${issue.priority[0].toUpperCase()}${issue.priority.slice(1)}</span>` : ""}</div>
    <a class="issue-card-title">${issue.title}</a>
    <div class="issue-card-labels">${issue.labels.map((label) => `<span class="issue-label">${label}</span>`).join("")}</div>
    <button class="issue-assign" type="button">${assignee(issue.assignee)}</button>
  </li>`;

const boardColumn = ([status, title]) => {
  const cards = ISSUES.filter((issue) => issue.status === status);
  return `<section class="issue-column" data-column="${status}">
    <header class="issue-column-head"><h3>${title}</h3><details class="issue-column-why"><summary>i</summary></details><span class="issue-column-count">${cards.length}</span></header>
    <ul class="issue-column-cards" role="list">${cards.map(issueCard).join("")}</ul>
  </section>`;
};

const projectPage = (content) => `<div id="project-pane" class="project-page">${content}</div>`;

const projectShell = ({ tab, root, inbox = INBOX_TEAM, rail = agentRail({ strip: {} }) }) => appShell({
  inbox,
  toolbar: projectToolbar(tab === "Workflows" ? [...PROJECT_TABS, "Workflows"] : PROJECT_TABS, tab, PROJECT_VERBS),
  root,
  rail,
  console: false,
});

export const boardScene = () => projectShell({
  tab: "Issues",
  inbox: INBOX_BEFORE_TEAM,
  root: projectPage(`${viewSwitch(["Dashboard", "List", "Board"], "Board", "New issue")}
    <div class="issue-filters" role="group">${filter("Open", true)}${filter("Any column")}${filter("Anyone")}${filter("Any label")}</div>
    <div class="issue-body"><div class="issue-board" role="group">${COLUMNS.map(boardColumn).join("")}</div></div>`),
});

// ---- the issue and its team ------------------------------------------------------

const railSection = (title, body, note = "") =>
  `<section class="issue-rail-section"><h2>${title}</h2>${body}${note ? `<p class="sub">${note}</p>` : ""}</section>`;

const issuePage = () => `<div id="issue-pane" class="issue-surface"><div class="issue-page">
    <div class="issue-page-main">
      <header class="issue-page-head">
        <div class="issue-page-marks"><span class="issue-state issue-state-open"></span><span class="issue-page-state">Open</span><span class="issue-number">#78</span><time class="issue-age">3m ago</time>
          <button type="button" class="iconbtn rail-watch watching" aria-pressed="true">${ICON_EYE}</button></div>
        <h1 class="issue-page-title">Make archived items searchable</h1>
        <div class="issue-page-labels"><span class="issue-label">search</span><span class="issue-label">bug</span></div>
      </header>
      <div class="issue-page-body markdown"><p>Archiving an item removes it from search. Keep archived items searchable and label them clearly.</p></div>
      <ul class="issue-timeline">
        <li class="issue-entry issue-event"><span class="issue-event-dot"></span><span class="issue-event-text"><strong>You</strong> filed this</span><span class="issue-when">3m ago</span></li>
        <li class="issue-entry issue-event"><span class="issue-event-dot"></span><span class="issue-event-text"><strong>You</strong> assigned Claude Code</span><span class="issue-when">2m ago</span></li>
        <li class="issue-entry issue-event"><span class="issue-event-dot"></span><span class="issue-event-text"><strong>Claude Code</strong> brought in Review and Audit edge cases</span><span class="issue-when">now</span></li>
      </ul>
    </div>
    <aside class="issue-rail" aria-label="About this issue">
      ${railSection("State", '<button class="btn" type="button">Close issue</button>')}
      ${railSection("Column", '<label class="create-label">Column</label><select><option>In progress</option></select>')}
      ${railSection("Priority", '<label class="create-label">Priority</label><select><option>High</option></select>')}
      ${railSection("Assignee", `<button class="btn issue-assign-open" type="button">${assignee("Claude Code")}</button>`)}
      ${railSection("Links", '<p class="sub mono">archive-search · archive-review · archive-audit</p>')}
    </aside>
  </div></div>`;

const teamOverview = () => overviewPanel({
  title: "Agents · 3 working",
  sections: [
    overviewSection("Project agents", "", { add: false }),
    ...TEAM.map((agent) => overviewSection(agent.workspace, overviewRow({ name: agent.name, snippet: agent.snippet, state: "Working" }))),
  ].join(""),
});

export const teamScene = () => projectShell({
  tab: "Issues",
  root: issuePage(),
  rail: agentRail({ overview: teamOverview(), strip: {} }),
});

// ---- the workflow builder (not in the app yet) --------------------------------------

const STEPS = [
  { title: "Implement", owner: "Claude Code", state: "Done · handoff pending", kind: "done" },
  { title: "Review", owner: "Codex", state: "Waiting" },
  { title: "You decide", owner: "Human gate", state: "Waiting", kind: "gate" },
  { title: "Merge", owner: "Build", state: "Waiting" },
];

const step = ({ title, owner, state, kind = "" }, index) =>
  `${index ? '<span class="workflow-arrow">→</span>' : ""}<article class="workflow-step ${kind}"><small>${index + 1}</small><strong>${title}</strong><span>${owner}</span><em>${state}</em></article>`;

export const builderScene = () => projectShell({
  tab: "Workflows",
  root: projectPage(`${viewSwitch(["Fix with tests", "Hotfix", "Release"], "Fix with tests", "New workflow")}
    <div class="workflow-builder">
      <aside class="workflow-palette"><h3>Add a step</h3>${["Agent", "Test", "Review", "Human gate", "Fan out"].map((label) => `<button class="workflow-palette-item" type="button">＋ ${label}</button>`).join("")}</aside>
      <div class="workflow-canvas"><div class="workflow-canvas-label">Archive search workflow</div><div class="workflow-path">${STEPS.map(step).join("")}</div></div>
    </div>`),
});
