// The demonstration data every landing screen draws: one project, the
// archive-search task, and the three workspaces its agents run in. Plain
// data, shaped the way the SPA's own renderers read it.

export const PROJECT = { id: "project-build", name: "build", projectKey: "workshop/project-build" };
export const DEVICE = "dev-mbp";
export const DIRECTORY = "build-app";
export const BRANCH = "build/archive-search";

// A workspace with a conversation waiting on you is still running; one whose
// agents are at work is ready, which is when the app offers Done.
const workspace = (id, name, summary, updated, status = "ready") => ({
  id: `workspace-${id}`,
  workspaceKey: `workshop/workspace-${id}`,
  project_id: PROJECT.id,
  projectKey: PROJECT.projectKey,
  deviceId: "workshop",
  entity_id: `run-${id}`,
  name,
  root: `/workspaces/${name}`,
  status,
  work_summary: summary,
  directories: [{ source_id: DIRECTORY, name: DIRECTORY, is_git: true }],
  created_at: "2026-09-17T20:00:00Z",
  updated_at: updated,
});

const conversation = (id, { working = false, unread = 0 } = {}) => ({
  kind: "branch",
  entity_id: `run-${id}`,
  project_id: PROJECT.id,
  projectKey: PROJECT.projectKey,
  branch: `build/${id}`,
  working,
  unread: unread > 0,
  unread_count: unread,
});

// Everything the inbox can list, newest first; each scene picks its rows.
export const WORKSPACES = {
  search: workspace("archive-search", "archive-search", { pushes: 2, pulls: 0, additions: 23, deletions: 1, clean: false }, "2026-09-17T20:03:46Z"),
  review: workspace("archive-review", "archive-review", { pushes: 1, pulls: 0, additions: 14, deletions: 0, clean: false }, "2026-09-17T20:03:45Z"),
  audit: workspace("archive-audit", "archive-audit", { pushes: 1, pulls: 0, additions: 6, deletions: 0, clean: false }, "2026-09-17T20:03:44Z"),
  terminals: workspace("terminal-sessions", "terminal-sessions", { pushes: 3, pulls: 1, additions: 88, deletions: 12, clean: false }, "2026-09-17T19:40:00Z", "active"),
  pairing: workspace("pairing-docs", "pairing-docs", { pushes: 0, pulls: 0, additions: 41, deletions: 3, clean: false }, "2026-09-17T19:10:00Z", "active"),
};

export const CONVERSATIONS = {
  search: conversation("archive-search", { working: true }),
  review: conversation("archive-review", { working: true }),
  audit: conversation("archive-audit", { working: true }),
  terminals: conversation("terminal-sessions", { unread: 2 }),
  pairing: conversation("pairing-docs", { unread: 1 }),
};

// Board order: the columns the tracker draws, and the card in each.
export const TASKS = [
  { number: 76, title: "Explain local host pairing", status: "backlog", labels: ["docs"] },
  { number: 78, title: "Make archived items searchable", status: "ready", labels: ["search", "bug"], priority: "high" },
  { number: 74, title: "Persist terminal sessions", status: "in_progress", labels: ["shell"], assignee: "Codex" },
  { number: 73, title: "Show branch divergence", status: "in_review", labels: ["git"], assignee: "Claude Code" },
];

export const COLUMNS = [
  ["backlog", "Backlog"],
  ["ready", "Ready"],
  ["in_progress", "In progress"],
  ["in_review", "In review"],
];

// The three agents act 4 fans the task out to, one per workspace.
export const TEAM = [
  { name: "Implement", harness: "claude", workspace: "archive-search", snippet: "Patching archiveItem so archived entries stay indexed." },
  { name: "Review", harness: "codex", workspace: "archive-review", snippet: "Reading the search index flow before the patch lands." },
  { name: "Audit edge cases", harness: "codex", workspace: "archive-audit", snippet: "Checking restore, re-archive and empty-query cases." },
];

export const TEST_FILE = [
  'import { archive, search } from "../search";',
  "",
  'describe("archive", () => {',
  '  const archived = archive("launch notes");',
  "",
  "});",
];

export const COMMITS = [
  { hash: "c41e2a9", short: "c41e2a9", subject: "Archive search behavior", author: "Claude Code", time: 1790213000 },
  { hash: "9b03f17", short: "9b03f17", subject: "Review archive coverage", author: "Codex", time: 1790212700 },
  { hash: "5d8a6e0", short: "5d8a6e0", subject: "Audit edge cases", author: "Codex", time: 1790212400 },
  { hash: "850ab6f", short: "850ab6f", subject: "Merge #74: persist terminal sessions", author: "Build", time: 1790209000 },
];
export const NOW_SECONDS = 1790213180;

export const ARCHIVE_DIFF = [
  { t: "hunk", text: "@@ -10,5 +10,7 @@ export function archiveItem(id: string) {" },
  { t: "ctx", o: 11, n: 11, text: "  const item = search.get(id)" },
  { t: "del", o: 12, text: "  search.remove(id)" },
  { t: "add", n: 12, text: "  search.update(id, {" },
  { t: "add", n: 13, text: "    archived: true," },
  { t: "add", n: 14, text: "  })" },
];

export const TEST_DIFF = [
  { t: "hunk", text: '@@ -3,4 +3,7 @@ describe("archive", () => {' },
  { t: "ctx", o: 4, n: 4, text: '  const archived = archive("launch notes");' },
  { t: "add", n: 6, text: '  it("keeps archived items in search", () => {' },
  { t: "add", n: 7, text: '    expect(search("launch notes")).toContain(archived);' },
  { t: "add", n: 8, text: "  });" },
];

// The hero's screen: the project's Tasks dashboard on Needs you, holding the
// three requests the landing's notification field converges on. The rows'
// reasons are the app's own (core/trackerAttentionModel.js, the earlier
// rule): a task In review, one assigned to you, and one with an unread agent
// comment from a watched task's inbox row.
const heroTask = (number, title, status, assignee, extra = {}) => ({
  id: `task-${number}`, number, title, status, state: "open", priority: "medium", assignee,
  identities: {}, labels: [], links: { commits: [] }, ...extra,
});
const heroAgent = (id) => ({ kind: "agent", agent_id: id });
export const HERO_TASKS = [
  heroTask(82, "Show branch divergence in the toolbar", "in_review", heroAgent("agent-review"), { unread_count: 1 }),
  heroTask(85, "Move session tokens to the new store", "in_progress", { kind: "user" }, { unread_count: 1 }),
  heroTask(86, "Rank search results by recency or relevance", "in_progress", null, { watched: true, unread_count: 2, read_through: "tc-01M3TA00000000000000000000" }),
  heroTask(83, "Remember expanded folders in the file tree", "in_progress", heroAgent("agent-tree")),
  heroTask(84, "Retry dropped uploads", "in_progress", heroAgent("agent-upload")),
  heroTask(87, "Keyboard moves between board columns", "ready", null),
  heroTask(88, "Offline banner for the files pane", "backlog", null),
  heroTask(89, "Explain local host pairing", "backlog", null),
];
// Which hero row each of the landing's attention pills lands on.
export const HERO_ROWS = { review: "task-82", approval: "task-85", question: "task-86" };
export const HERO_PROJECT_KEY = "workshop|project-build";
export const HERO_FEED = {
  projects: [{ projectKey: HERO_PROJECT_KEY, name: PROJECT.name }],
  items: [
    { projectKey: HERO_PROJECT_KEY, entity_id: "run-archive-search", agents: [
      { id: "agent-review", name: "Review", working: false },
      { id: "agent-tree", name: "Implement", working: true },
      { id: "agent-upload", name: "Upload fixer", working: true },
    ] },
    { projectKey: HERO_PROJECT_KEY, kind: "tracker_task", task_id: "task-86", unread: 2 },
  ],
};
export const HERO_DETAILS = new Map([["task-86", {
  task: { id: "task-86", read_through: "tc-01M3TA00000000000000000000" },
  timeline: [{ id: "tc-01M3TB00000000000000000000", type: "comment", author: { kind: "agent", agent_id: "agent-pi" }, body: "Which approach do you want?" }],
}]]);
