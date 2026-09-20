import { readFileSync, writeFileSync } from "node:fs";
const SPA = "/home/zech/Projects/.build-worktrees/workspaces/proj-1/tracker-filters/Build/spa";
const sheet = (n) => readFileSync(`${SPA}/src/${n}`, "utf8");
const OPTIONS = {
  states: [{value:"",label:"Open and closed"},{value:"open",label:"Open"},{value:"closed",label:"Closed"}],
  statuses: [{value:"",label:"Any column"},{value:"backlog",label:"Backlog"},{value:"ready",label:"Ready"},{value:"in_progress",label:"In progress"},{value:"in_review",label:"In review"},{value:"done",label:"Done"}],
  assignees: [
    {value:"",label:"Anyone"},{value:"any",label:"Anyone assigned"},{value:"none",label:"Unassigned"},{value:"user",label:"You"},
    {value:"agent:a1",label:"issues-spa · Agent 1",group:"issues-spa"},
    {value:"agent:a2",label:"issues-spa · Agent 2",group:"issues-spa"},
    {value:"agent:b1",label:"tracker-filters · Agent 1",group:"tracker-filters"},
    {value:"agent:c1",label:"transport-liveness · Agent 1",group:"transport-liveness"},
  ],
  labels: [{value:"",label:"Any label"},{value:"bridge",label:"bridge"},{value:"bug",label:"bug"},{value:"perf",label:"perf"},{value:"prompts",label:"prompts"},{value:"spa",label:"spa"},{value:"tracker",label:"tracker"},{value:"transport",label:"transport"},{value:"ux",label:"ux"},{value:"wire",label:"wire"}],
};
const FILTERS = { state: "open", status: "", assignee: ["agent:b1", "user"], label: ["spa", "tracker"] };
const page = (theme, width, tall) => `<!doctype html><html lang="en" data-theme="${theme}"><head><meta charset="utf-8">
<style>${sheet("styles.css").replace(/@import[^;]+;/g, "")}</style>
<style>${sheet("styles/issues.css")}</style>
<style>html{background:var(--bg)}body{margin:0;width:${width}px}
.surface{background:var(--panel);border-radius:12px;margin:10px;overflow:visible;min-height:${tall}px}</style>
</head><body><div class="surface"><div id="pane"></div></div>
<script type="module">
import { mountIssuesChrome } from "./src/core/trackerPaneChrome.js";
const chrome = mountIssuesChrome(document.querySelector("#pane"), { onView(){}, onNew(){}, onFilter(){}, onClear(){} });
chrome.update({ view: "list", filters: ${JSON.stringify(FILTERS)}, options: ${JSON.stringify(OPTIONS)} });
window.__ready = true;
</script></body></html>`;
writeFileSync(`${SPA}/__menu-390.html`, page("dark", 390, 720));
writeFileSync(`${SPA}/__menu-1440.html`, page("dark", 1440, 440));
writeFileSync(`${SPA}/__menu-1440-light.html`, page("light", 1440, 440));
console.log("pages written");
