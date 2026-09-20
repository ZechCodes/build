// Renders the Issues-tab list from a given spa/ checkout — the real modules,
// the real sheet — into one standalone page, so a screenshot is of the code
// and not of a mock. Takes the spa root and an output prefix.
import { writeFileSync, readFileSync } from "node:fs";

const SPA = process.argv[2]; // …/spa — this checkout, or another one extracted from a sha
const PREFIX = process.argv[3];
const { issueRowHtml } = await import(`${SPA}/src/core/trackerListRender.js`);

const columns = [
  { id: "backlog", name: "Backlog" },
  { id: "ready", name: "Ready" },
  { id: "in_progress", name: "In progress" },
  { id: "in_review", name: "In review" },
  { id: "done", name: "Done" },
];

const NOW = Date.parse("2026-09-20T20:00:00Z");
const ago = (hours) => new Date(NOW - hours * 3600_000).toISOString();

const AGENTS = {
  "agent-1": "issues-spa · Agent 1",
  "agent-2": "tracker-filters · Agent 2",
  "agent-3": "transport-liveness · Agent 1",
};

const ISSUES = [
  { number: 45, title: "The issue list still reads as cluttered: a quieter row", status: "in_progress", priority: "high", labels: ["spa", "tracker", "ux"], assignee: { kind: "agent", agent_id: "agent-2" }, updated_at: ago(0.2) },
  { number: 44, title: "Custom filter dropdowns for the Issues tab: multi-select with fuzzy search for labels and assignees", status: "ready", priority: "medium", labels: ["spa", "tracker", "ux"], assignee: null, updated_at: ago(0.6) },
  { number: 43, title: "Issues tab: the filter bar is mounted once and never redrawn", status: "in_review", priority: "urgent", labels: ["spa", "tracker", "ux", "perf"], assignee: { kind: "agent", agent_id: "agent-2" }, updated_at: ago(1) },
  { number: 41, title: "A stall harness for the relayed path", status: "in_progress", priority: "none", labels: ["bridge", "transport"], assignee: { kind: "agent", agent_id: "agent-3" }, updated_at: ago(3) },
  { number: 40, title: "Notice rows in the issues SPA read as messages from nobody", status: "ready", priority: "medium", labels: ["spa"], assignee: { kind: "agent", agent_id: "agent-1" }, updated_at: ago(5) },
  { number: 39, title: "Prompt delivery drops the second attachment when the bridge reconnects mid-send", status: "backlog", priority: "high", labels: ["bridge", "prompts", "wire"], assignee: null, updated_at: ago(9) },
  { number: 38, title: "Ghost rows survive a workspace being removed", status: "done", priority: "low", labels: ["spa"], assignee: { kind: "user" }, updated_at: ago(26) },
  { number: 33, title: "Closed issues should not be in the default view", status: "done", priority: "none", labels: ["tracker", "ux"], assignee: { kind: "user" }, state: "closed", updated_at: ago(30) },
  { number: 31, title: "ICE preference: prefer the direct path when both are up", status: "in_progress", priority: "medium", labels: ["transport"], assignee: { kind: "agent", agent_id: "agent-3" }, updated_at: ago(48) },
  { number: 30, title: "Reconnect storms after a laptop lid closes", status: "in_review", priority: "urgent", labels: ["transport", "bridge"], assignee: { kind: "agent", agent_id: "agent-3" }, updated_at: ago(52) },
  { number: 23, title: "A worktree removed by hand outlives its row in the picker", status: "backlog", priority: "none", labels: ["bridge", "workspaces", "ux", "debt"], assignee: null, updated_at: ago(96) },
  { number: 19, title: "Shell calls from an adopted checkout run in the wrong directory", status: "backlog", priority: "high", labels: ["shell"], assignee: null, updated_at: ago(140) },
];

const issue = (over) => ({
  id: `issue-${over.number}`, project_id: "proj-1", number: 1, title: "", body: "",
  state: "open", status: "backlog", labels: [], priority: "none", assignee: null,
  links: {}, created_by: { kind: "user" }, created_at: ago(200), updated_at: ago(1), closed_at: null,
  ...over,
});

const rows = ISSUES.map((one) =>
  issueRowHtml(issue(one), { columns, agentLabels: AGENTS, href: (i) => `#/issues/${i.id}`, nowMs: NOW }),
).join("");

const sheet = (name) => readFileSync(`${SPA}/src/${name}`, "utf8");

const page = (theme, width) => `<!doctype html><html lang="en" data-theme="${theme}"><head><meta charset="utf-8">
<style>${sheet("styles.css").replace(/@import[^;]+;/g, "")}</style>
<style>${sheet("styles/issues.css")}</style>
<style>
  html { background:var(--bg); }
  body { margin:0; width:${width}px; }
  .surface { background:var(--panel); border-radius:12px; margin:10px; overflow:hidden; }
</style></head>
<body><div class="surface">
  <div class="issue-head">
    <div class="issue-views" role="group">
      <button class="btn mini issue-view active" type="button" aria-pressed="true">List</button>
      <button class="btn mini issue-view" type="button" aria-pressed="false">Board</button>
    </div>
    <button class="btn mini primary issue-new" type="button"><span>New issue</span></button>
  </div>
  <div class="issue-filters" role="group">
    <select class="issue-filter"><option>Open</option></select>
    <select class="issue-filter"><option>Any column</option></select>
    <select class="issue-filter"><option>Anyone</option></select>
    <select class="issue-filter"><option>Any label</option></select>
  </div>
  <div class="issue-body"><ul class="issue-rows">${rows}</ul></div>
</div></body></html>`;

for (const [theme, width, name] of [["dark", 390, "390-dark"], ["dark", 1440, "1440-dark"], ["light", 390, "390-light"], ["light", 1440, "1440-light"]]) {
  writeFileSync(`/tmp/rowshot/${PREFIX}-${name}.html`, page(theme, width));
}
console.log(`${PREFIX} ok`);
