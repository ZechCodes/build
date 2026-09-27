// The scenes that stand on the project's own pages: the Tasks board (act 3),
// the task with its three agents in the chat overview (act 4), and the
// workflow builder (act 5, a feature the app does not have yet, drawn as a
// third project page in the app's frame).
import { ICON_CHEVRON_DOWN, ICON_EYE, ICON_PLUS } from "../../spa/src/core/icons.js";
import { stateMarkHtml } from "../../spa/src/core/trackerChips.js";
import {
  PROJECT_VERBS, agentRail, appShell, harnessMark, overviewPanel, overviewRow, overviewSection, projectToolbar,
} from "./app-shell.js";
import { INBOX_TEAM } from "./workspace-scenes.js";
import { COLUMNS, TASKS, TEAM } from "./story.js";

const INBOX_BEFORE_TEAM = ["search", "terminals", "pairing"];
const PROJECT_TABS = ["Tasks", "Workspaces"];

const caret = ICON_CHEVRON_DOWN.replace('class="lucide', 'class="fmenu-caret lucide');
const filter = (label, set = false) =>
  `<div class="fmenu"><button class="fmenu-press${set ? " is-set" : ""}" type="button"><span class="fmenu-press-label">${label}</span>${caret}</button></div>`;

const viewSwitch = (views, active, action) => `<div class="task-head">
    <div class="task-views" role="group">${views.map((view) => `<button class="btn mini task-view${view === active ? " active" : ""}" type="button">${view}</button>`).join("")}</div>
    <button class="btn mini primary task-new" type="button">${ICON_PLUS}<span>${action}</span></button>
  </div>`;

const assignee = (name) => name
  ? `<span class="task-assignee"><span class="task-avatar is-harness" aria-hidden="true">${harnessMark(name === "Codex" ? "codex" : "claude")}</span>${name}</span>`
  : '<span class="task-assignee task-unassigned">Unassigned</span>';

const taskCard = (task) => `<li class="task-card" data-status="${task.status}" data-task-number="${task.number}">
    <div class="task-card-head">${stateMarkHtml({ state: "open", status: task.status })}<span class="task-number">#${task.number}</span>${task.priority ? `<span class="task-priority task-priority-${task.priority}">${task.priority[0].toUpperCase()}${task.priority.slice(1)}</span>` : ""}</div>
    <a class="task-card-title">${task.title}</a>
    <div class="task-card-labels">${task.labels.map((label) => `<span class="task-label">${label}</span>`).join("")}</div>
    <button class="task-assign" type="button">${assignee(task.assignee)}</button>
  </li>`;

const boardColumn = ([status, title]) => {
  const cards = TASKS.filter((task) => task.status === status);
  return `<section class="task-column" data-column="${status}">
    <header class="task-column-head"><h3>${title}</h3><details class="task-column-why"><summary>i</summary></details><span class="task-column-count">${cards.length}</span></header>
    <ul class="task-column-cards" role="list">${cards.map(taskCard).join("")}</ul>
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
  tab: "Tasks",
  inbox: INBOX_BEFORE_TEAM,
  root: projectPage(`${viewSwitch(["Dashboard", "List", "Board"], "Board", "New task")}
    <div class="task-filters" role="group">${filter("Open", true)}${filter("Any column")}${filter("Anyone")}${filter("Any label")}</div>
    <div class="task-body"><div class="task-board" role="group">${COLUMNS.map(boardColumn).join("")}</div></div>`),
});

// ---- the task and its team ------------------------------------------------------

const railSection = (title, body, note = "") =>
  `<section class="task-rail-section"><h2>${title}</h2>${body}${note ? `<p class="sub">${note}</p>` : ""}</section>`;

const taskPage = () => `<div id="task-pane" class="task-surface"><div class="task-page">
    <div class="task-page-main">
      <header class="task-page-head">
        <div class="task-page-marks">${stateMarkHtml({ state: "open", status: "ready" })}<span class="task-page-state">Open</span><span class="task-number">#78</span><time class="task-age">3m ago</time>
          <button type="button" class="iconbtn rail-watch watching" aria-pressed="true">${ICON_EYE}</button></div>
        <h1 class="task-page-title">Make archived items searchable</h1>
        <div class="task-page-labels"><span class="task-label">search</span><span class="task-label">bug</span></div>
      </header>
      <div class="task-page-body markdown"><p>Archiving an item removes it from search. Keep archived items searchable and label them clearly.</p></div>
      <ul class="task-timeline">
        <li class="task-entry task-event"><span class="task-event-dot"></span><span class="task-event-text"><strong>You</strong> filed this</span><span class="task-when">3m ago</span></li>
        <li class="task-entry task-event"><span class="task-event-dot"></span><span class="task-event-text"><strong>You</strong> assigned Claude Code</span><span class="task-when">2m ago</span></li>
        <li class="task-entry task-event"><span class="task-event-dot"></span><span class="task-event-text"><strong>Claude Code</strong> brought in Review and Audit edge cases</span><span class="task-when">now</span></li>
      </ul>
    </div>
    <aside class="task-rail" aria-label="About this task">
      ${railSection("State", '<button class="btn" type="button">Close task</button>')}
      ${railSection("Column", '<label class="create-label">Column</label><select><option>In progress</option></select>')}
      ${railSection("Priority", '<label class="create-label">Priority</label><select><option>High</option></select>')}
      ${railSection("Assignee", `<button class="btn task-assign-open" type="button">${assignee("Claude Code")}</button>`)}
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
  tab: "Tasks",
  root: taskPage(),
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
