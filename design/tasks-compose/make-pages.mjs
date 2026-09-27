// The Tasks tab with the composer open, mounted the way the pane mounts it —
// real chrome, real composer, real sheet — so the picture is of the code.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
const SPA = fileURLToPath(new URL("../../spa", import.meta.url));
const sheet = (n) => readFileSync(`${SPA}/src/${n}`, "utf8");

const OPTIONS = {
  states: [{value:"",label:"Open and closed"},{value:"open",label:"Open"},{value:"closed",label:"Closed"}],
  statuses: [{value:"",label:"Any column"},{value:"backlog",label:"Backlog"},{value:"ready",label:"Ready"},{value:"in_progress",label:"In progress"},{value:"in_review",label:"In review"},{value:"done",label:"Done"}],
  assignees: [{value:"",label:"Anyone"},{value:"any",label:"Anyone assigned"},{value:"none",label:"Unassigned"},{value:"user",label:"You"}],
  labels: [{value:"",label:"Any label"},{value:"bug",label:"bug"},{value:"spa",label:"spa"},{value:"tracker",label:"tracker"},{value:"ux",label:"ux"}],
};
const ASSIGNEE_OPTIONS = [
  { id: "none", kind: "unassign", label: "Unassigned", hint: "Nobody holds it. Nothing running is stopped.", group: "" },
  { id: "user", kind: "user", label: "You", hint: "Nothing is dispatched.", group: "" },
  { id: "project_agent", kind: "project_agent", label: "Project agent", hint: "Hands it to this project's own agent and starts it.", group: "" },
  { id: "agent:a1", kind: "agent", agentId: "a1", workspaceId: "ws-1", label: "tasks-spa · Agent 1", hint: "Delivers the task into this agent's conversation.", group: "tasks-spa" },
  { id: "new_workspace", kind: "new_workspace", label: "New workspace and agent", hint: "Cuts a workspace in this project and starts an agent on it.", group: "", form: "workspace" },
];
const ROWS = [
  { number: 45, title: "The task list still reads as cluttered: a quieter row", status: "in_progress", priority: "high", labels: ["spa","tracker","ux"], updated_at: "2026-09-20T19:50:00Z" },
  { number: 44, title: "Custom filter dropdowns for the Tasks tab", status: "ready", priority: "medium", labels: ["spa","tracker"], updated_at: "2026-09-20T19:20:00Z" },
  { number: 43, title: "Tasks tab: the filter bar is mounted once", status: "in_review", priority: "urgent", labels: ["spa","ux"], updated_at: "2026-09-20T19:00:00Z" },
];

const page = (theme, width, tall) => `<!doctype html><html lang="en" data-theme="${theme}"><head><meta charset="utf-8">
<style>${sheet("styles.css").replace(/@import[^;]+;/g, "")}</style>
<style>${sheet("styles/tasks.css")}</style>
<style>html{background:var(--bg)}body{margin:0;width:${width}px}
.surface{background:var(--panel);border-radius:12px;margin:10px;overflow:visible;min-height:${tall}px}</style>
</head><body><div class="surface"><div id="pane"></div></div>
<script type="module">
import { mountTasksChrome } from "./src/core/trackerPaneChrome.js";
import { paintTaskRows } from "./src/core/trackerTasksBody.js";
import { openTaskComposer } from "./src/core/taskComposer.js";
const chrome = mountTasksChrome(document.querySelector("#pane"), { onView(){}, onNew(){}, onFilter(){}, onClear(){} });
chrome.update({ view: "list", filters: { state: "open" }, options: ${JSON.stringify(OPTIONS)} });
const task = (o) => ({ id: "i"+o.number, state: "open", labels: [], priority: "none", assignee: null, ...o });
paintTaskRows(chrome.body, ${JSON.stringify(ROWS)}.map(task), {
  columns: null, agentLabels: {}, href: (i) => "#/"+i.id, filters: { state: "open" },
  nowMs: Date.parse("2026-09-20T20:00:00Z"),
});
openTaskComposer(chrome.composeSlot, {
  projectId: "proj-1", projectName: "Build", columns: null,
  labels: ["bug", "spa", "tracker", "ux"],
  options: ${JSON.stringify(ASSIGNEE_OPTIONS)},
  catalog: { default_provider: "claude", providers: [] },
  callRpc: async (method, params) => method === "tasks.attach"
    ? { name: params.filename, path: ".build/attachments/abc-" + params.filename, mime: params.filename.endsWith(".png") ? "image/png" : "text/plain", size: params.filename.endsWith(".png") ? 212000 : 84000 }
    : {},
});
const typeInto = (selector, value) => {
  const field = document.querySelector(selector);
  field.value = value;
  field.dispatchEvent(new Event("input"));
};
typeInto("#task-new-summary", "Kanban drag does not persist after a reload");
typeInto("#task-new-body", ["Dragging a card to In review leaves it where it was after a reload.", "", "Screenshot and the trace attached."].join(String.fromCharCode(10)));
// Real files through the real tray, so the chips are the code's and not a
// drawing of them.
const drop = (files) => {
  const event = new Event("drop", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: { files } });
  document.querySelector(".task-compose").dispatchEvent(event);
};
drop([
  new File([new Uint8Array(2048)], "shot.png", { type: "image/png" }),
  new File(["a stack trace"], "trace.log", { type: "text/plain" }),
]);
setTimeout(() => { window.__ready = true; }, 400);
</script></body></html>`;

writeFileSync(`${SPA}/__compose-390.html`, page("dark", 390, 760));
writeFileSync(`${SPA}/__compose-1440.html`, page("dark", 1440, 620));
console.log("pages written");
