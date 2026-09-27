// One project on disk for the agents overview (#186), the way the sync layer
// writes it: the Skrift project as the maintainer's screenshot showed it. Four
// workspaces and six agents in mixed states — one working, one that failed to
// start, two unread (one of them blocked), one idle whose last word is a
// markdown table, and the project's own — and one workspace nobody has spoken
// in yet. Shared by the jsdom checks and the browser captures, so both stand
// on the same picture; the writers are handed in because each environment
// loads the modules its own way.

export const OVERVIEW_DEVICE = "skrift-device";
export const OVERVIEW_PROJECT = "skrift-project";

const at = (day, hour, minute) => new Date(Date.UTC(2026, 8, day, hour, minute)).toISOString();

const agent = (id, ordinal, over = {}) => ({
  id, ordinal, provider: "claude_adk", model: "", active_model: "", effort: "", state: "live",
  unread_count: 0, read_through_sequence: 0, working: false, watched: true, ...over,
});

const said = (sequence, body, createdAt) => ({ type: "message", data: { sequence, role: "agent", body, created_at: createdAt } });
const called = (sequence, summary, createdAt) => ({ type: "event", data: { sequence, event: "tool_use", summary, created_at: createdAt } });

export const AUDIT_TABLE = [
  "| Issue | Verdict | Evidence / remaining work |",
  "|---|---|---|",
  "| #183 | **Done** | Worker cancels jobs on `SIGTERM`; see [PR 12](https://example.test/pr/12) |",
  "| #168 | _Partial_ | Claims held 1800s |",
].join("\n");

/** The rows the overview reads: the project's agent and each workspace's. */
export const OVERVIEW_ROWS = [
  { entityId: "project-run", kind: "project", workspaceId: null, agents: [
    agent("project-agent", 1, { active_model: "claude-fable-5-1" }),
  ] },
  { entityId: "run-fixes", kind: "workspace", workspaceId: "ws-fixes", agents: [
    agent("fixer", 1, { name: "Issue fixer", active_model: "claude-opus-5", working: true }),
    agent("fix-reviewer", 2, { name: "Fix reviewer", provider: "codex", active_model: "gpt-6-sol",
      start_error: "codex: model alias \"sol\" is not a model id" }),
  ] },
  { entityId: "run-review", kind: "workspace", workspaceId: "ws-review", agents: [
    agent("pr-reviewer", 1, { name: "PR reviewer", provider: "codex", active_model: "gpt-6-sol", watched: false,
      unread_count: 1, unread_reason: "agent_message" }),
    agent("critical-review", 2, { name: "Critical review", provider: "codex", active_model: "gpt-6-astra", effort: "xhigh",
      watched: false, unread_count: 2, unread_reason: "blocked" }),
  ] },
  { entityId: "run-audit", kind: "workspace", workspaceId: "ws-audit", agents: [
    agent("auditor", 1, { name: "Issue auditor", provider: "codex", active_model: "gpt-6-sol", watched: false }),
  ] },
];

/** What each agent last said or did. The auditor's is the newest, so an
 *  overview ordered by last word alone would put its quiet workspace first. */
export const OVERVIEW_THREADS = {
  "project-agent": [said(1, "I have the review brief and will wait for the first PR number before starting.", at(27, 14, 40))],
  fixer: [said(1, "Starting on #183.", at(27, 14, 50)), called(2, "Running `pytest skrift/tests/test_worker.py -q`", at(27, 14, 52))],
  "fix-reviewer": [],
  "pr-reviewer": [said(1, "Awaiting the first PR number; no review has started.", at(27, 14, 55))],
  "critical-review": [said(1, "Review pending PR assignment. No review or checks have run yet.", at(27, 14, 56))],
  auditor: [said(1, AUDIT_TABLE, at(27, 15, 5))],
};

export const OVERVIEW_WORKSPACES = [
  { id: "ws-fixes", project_id: OVERVIEW_PROJECT, entity_id: "run-fixes", name: "skrift-fixes" },
  { id: "ws-review", project_id: OVERVIEW_PROJECT, entity_id: "run-review", name: "skrift-review" },
  { id: "ws-audit", project_id: OVERVIEW_PROJECT, entity_id: "run-audit", name: "issue-implementation-audit" },
  { id: "ws-soak", project_id: OVERVIEW_PROJECT, name: "relay-soak" },
];

const issue = (number, title, status, over = {}) => ({
  id: `issue-${number}`, project_id: OVERVIEW_PROJECT, number, title, body: "", state: "open", status,
  labels: [], priority: "normal", assignee: null,
  links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_issue_id: null },
  created_by: { kind: "user" }, created_at: at(26, 9, 0), updated_at: at(27, 12, 0), closed_at: null, ...over,
});

/** The project's issues as the tracker cached them: one linked to a workspace,
 *  one held by an agent in another, one finished and so not shown. */
export const OVERVIEW_ISSUES = [
  issue(183, "Worker shutdown cancels running jobs on SIGTERM", "in_progress",
    { links: { workspace_ids: ["ws-fixes"], branches: [], commits: [], conversation_ids: [], parent_issue_id: null } }),
  issue(1, "Skrift issue pipeline", "in_progress", { assignee: { kind: "agent", agent_id: "pr-reviewer" } }),
  issue(140, "Finished already", "done", { assignee: { kind: "agent", agent_id: "auditor" } }),
];

/** Put the whole project on disk. `writeCached` and `stampWorkspace` are the
 *  cache's own (core/localCache.js, core/feedMerge.js), and
 *  `writeIssuesRecord` the tracker's (core/trackerCache.js). */
export async function writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeIssuesRecord }) {
  const deviceId = OVERVIEW_DEVICE;
  for (const { entityId, kind, workspaceId, agents } of OVERVIEW_ROWS) {
    await writeCached({ deviceId, entityId, kind: "row", sub: "" }, {
      kind, entity_id: entityId, project_id: OVERVIEW_PROJECT,
      ...(workspaceId ? { workspace_id: workspaceId } : null), agents,
    });
    for (const one of agents) {
      await writeCached({ deviceId, entityId, kind: "thread", sub: one.id }, { items: OVERVIEW_THREADS[one.id] || [] });
    }
  }
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" },
    [{ id: OVERVIEW_PROJECT, project_id: OVERVIEW_PROJECT, name: "Skrift", entity_id: "project-run" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces", sub: "" },
    OVERVIEW_WORKSPACES.map((workspace) => stampWorkspace(workspace, deviceId)));
  await writeIssuesRecord(deviceId, OVERVIEW_PROJECT, { issues: OVERVIEW_ISSUES, columns: [
    { id: "backlog", name: "Backlog" }, { id: "in_progress", name: "In progress" }, { id: "done", name: "Done" },
  ] });
}

/** The context the project's page mounts its rail with. */
export const overviewRailContext = (call) => ({
  kind: "project", deviceId: OVERVIEW_DEVICE, projectId: OVERVIEW_PROJECT, entityId: "project-run",
  call: call || (async (method) => (method === "models.list"
    ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
    : { items: [] })),
});
