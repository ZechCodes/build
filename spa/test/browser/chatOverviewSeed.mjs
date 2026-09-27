// One project on disk for the chat overview's browser checks (#117): its own
// agent, the workspace the page stands on, a quiet workspace, a busy one with
// more agents than the project's overview shows, one whose agents have all
// gone and one that has never had any. Everything is written the
// way the sync layer writes it, and the rail mounted on it is the production
// one with nothing stood in.

import { loadBrowserModules } from "./layoutHarness.mjs";

/** The app first, and alone: its module graph has a cycle that only resolves
 *  when the app is the entry, and a parallel import that reaches the cycle
 *  from the rail's side first fails on a binding still in its dead zone. */
export async function loadChatOverviewModules(page, basePath, extra = {}) {
  await loadBrowserModules(page, { app: "src/app.js" }, basePath);
  await page.evaluate(() => { window.__appModule = window.__layoutModules.app; delete window.__layoutModules; });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js",
    cache: "src/core/localCache.js",
    feed: "src/core/taskFeed.js",
    merge: "src/core/feedMerge.js",
    ...extra,
  }, basePath);
  await page.evaluate(() => { window.__layoutModules.app = window.__appModule; });
}

/** Runs in the page: `page.evaluate(seedChatOverview)`. */
export async function seedChatOverview() {
  const { App } = window.__layoutModules.app;
  const { mountAgentRail } = window.__layoutModules.rail;
  const { writeCached } = window.__layoutModules.cache;
  const { startFeed } = window.__layoutModules.feed;
  const { stampWorkspace } = window.__layoutModules.merge;
  const deviceId = "overview-device";
  const projectId = "overview-project";
  App.devices = [{ id: deviceId, name: "This computer", status: "online" }];
  App.selectedDeviceId = deviceId;
  const minute = (at) => new Date(Date.UTC(2026, 8, 23, 20, at)).toISOString();
  const agentsOf = (entityId, list) => list.map(([id, name, topic, extra = {}], index) => ({
    id, name, topic, ordinal: index + 1, provider: index % 2 ? "codex" : "claude_adk", state: "live",
    unread_count: 0, working: false, watched: true, ...extra,
  }));
  const rows = [
    ["project-run", null, agentsOf("project-run", [["project-agent", "Build", "Orchestrate Build tasks"]])],
    ["current-run", "workspace-current", agentsOf("current-run", [
      ["current-agent", "Chat overview", "Chat overview navigation", { working: true }],
    ])],
    ["quiet-run", "workspace-quiet", agentsOf("quiet-run", [
      ["quiet-agent", "Landing copy", "Tighten the hero copy"],
      ["quiet-review", "Landing review", "Review the landing copy", { watched: false }],
    ])],
    ["busy-run", "workspace-busy", agentsOf("busy-run", [
      ["busy-1", "Bridge audit", "Audit bridge logic"],
      ["busy-2", "Flaky tests", "Fix flaky SPA tests", { unread_count: 2 }],
      ["busy-3", "Inbox order", "Inbox session order", { working: true }],
      ["busy-4", "Task unread", "Task unread line"],
      ["busy-5", "Comment links", "Comment and commit links"],
    ])],
    ["idle-run", "workspace-idle", []],
  ];
  const replies = {
    "project-agent": "Queued #117 behind #114; rolling after review.",
    "current-agent": "Rendering the overview scopes.",
    "quiet-agent": "Hero copy is down to two lines.",
    "quiet-review": "Two notes on the second paragraph.",
    "busy-1": "Five findings in the audit, filed as tasks.",
    "busy-2": "Three fixed-turn waits replaced with conditions.",
    "busy-3": "Anchor is the first message after a 12h gap.",
    "busy-4": "The unread line sits above the first new comment.",
    "busy-5": "Commit refs now link into the Changes tab.",
  };
  let at = 0;
  for (const [entityId, workspaceId, agents] of rows) {
    await writeCached({ deviceId, entityId, kind: "row", sub: "" }, {
      kind: workspaceId ? "workspace" : "project", entity_id: entityId, project_id: projectId,
      ...(workspaceId ? { workspace_id: workspaceId } : null), agents,
    });
    for (const agent of agents) {
      at += 1;
      await writeCached({ deviceId, entityId, kind: "thread", sub: agent.id }, { items: [{ type: "message",
        data: { sequence: 1, role: "agent", body: replies[agent.id], created_at: minute(at) } }] });
    }
  }
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" }, [{ id: projectId, project_id: projectId, name: "Build" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces", sub: "" }, [
    { id: "workspace-current", project_id: projectId, entity_id: "current-run", name: "chat-overview-nav" },
    { id: "workspace-quiet", project_id: projectId, entity_id: "quiet-run", name: "landing-page" },
    { id: "workspace-busy", project_id: projectId, entity_id: "busy-run", name: "spa-flaky-tests" },
    { id: "workspace-idle", project_id: projectId, entity_id: "idle-run", name: "relay-candidates" },
    { id: "workspace-new", project_id: projectId, name: "review-system-plan" },
  ].map((workspace) => stampWorkspace(workspace, deviceId)));
  await startFeed();
  window.__overviewRail = mountAgentRail(document.querySelector("#agent-rail"), {
    kind: "workspace", deviceId, projectId, workspaceId: "workspace-current", entityId: "current-run",
    projectAgent: { projectId, entityId: "project-run", name: "Build" },
    call: async (method) => method === "models.list"
      ? { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] }
      : { items: [] },
  });
}
