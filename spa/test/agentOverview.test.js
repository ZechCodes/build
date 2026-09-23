import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;
let overview;
let scope;

const settle = async () => {
  for (let turn = 0; turn < 6; turn += 1) await new Promise((done) => setTimeout(done, 0));
};
const message = (sequence, role, body) => ({ type: "message", data: { sequence, role, body } });
const activity = (sequence, summary) => ({ type: "event", data: { sequence, event: "tool_use", summary } });
const rosterAddress = { deviceId: "dev-overview", entityId: "run-overview", kind: "row", sub: "" };
const threadAddress = (id) => ({ deviceId: "dev-overview", entityId: "run-overview", kind: "thread", sub: id });
const workspaceListAddress = { deviceId: "dev-overview", entityId: "", kind: "workspaces" };
const rowAddress = (entityId) => ({ deviceId: "dev-overview", entityId, kind: "row", sub: "" });
const workspaceThreadAddress = (entityId, agentId) => ({ deviceId: "dev-overview", entityId,
  kind: "thread", sub: agentId });
const datedMessage = (sequence, role, body, createdAt) => ({ type: "message",
  data: { sequence, role, body, created_at: createdAt } });
const sectionNames = (rows) => [...overview.overviewHtml(rows).matchAll(
  /<section class="rail-overview-section" aria-label="([^"]+)"/g,
)].map((match) => match[1]);
const agent = (id, changes = {}) => ({ id, name: id, ordinal: 1, working: false,
  unread_count: 0, read_through_sequence: 0, ...changes });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  overview = await import("../src/core/agentOverview.js");
  scope = (await import("../src/core/cacheScope.js")).scopeFor("dev-overview");
});

describe("expanded agent overview", () => {
  it("groups a project's cached agents with its populated workspaces by latest agent message", async () => {
    const projectAddress = rowAddress("run-overview");
    await cache.writeCached(projectAddress, { kind: "project", agents: [agent("project-agent", { name: "Project agent" })] });
    await cache.writeCached(workspaceListAddress, [
      { id: "ws-first", project_id: "project-1", entity_id: "run-first", name: "First workspace" },
      { id: "ws-second", project_id: "project-1", entity_id: "run-second", name: "Second workspace" },
      { id: "ws-empty", project_id: "project-1", entity_id: "run-empty", name: "Empty workspace" },
      { id: "ws-other", project_id: "project-2", entity_id: "run-other", name: "Other project" },
    ]);
    await cache.writeCached(rowAddress("run-first"), { agents: [
      agent("first-older", { name: "First older" }), agent("first-newer", { name: "First newer" }),
    ] });
    await cache.writeCached(rowAddress("run-second"), { agents: [agent("second-agent", { name: "Second agent" })] });
    await cache.writeCached(rowAddress("run-empty"), { agents: [] });
    await cache.writeCached(rowAddress("run-other"), { agents: [agent("other-agent")] });
    await cache.writeCached(workspaceThreadAddress("run-first", "first-older"), { items: [
      datedMessage(1, "agent", "Older first reply", "2026-09-20T10:00:00Z"),
    ] });
    await cache.writeCached(workspaceThreadAddress("run-first", "first-newer"), { items: [
      datedMessage(1, "agent", "Newer first reply", "2026-09-21T10:00:00Z"),
    ] });
    await cache.writeCached(workspaceThreadAddress("run-second", "second-agent"), { items: [
      datedMessage(1, "agent", "Second reply", "2026-09-22T10:00:00Z"),
      datedMessage(2, "user", "Later user prompt", "2026-09-23T10:00:00Z"),
    ] });

    const paints = [];
    const reader = overview.createAgentOverview({ scope, projectId: "project-1", includeProjectWorkspaces: true,
      sources: () => [{ slot: "current", kind: "project", entityId: "run-overview", section: "project",
        sectionName: "Project agents", address: projectAddress }],
      onRows: (rows) => paints.push(rows),
    });
    try {
      reader.open();
      await vi.waitFor(() => expect(paints.at(-1)).toHaveLength(4));
      const rows = paints.at(-1);
      expect(sectionNames(rows)).toEqual(["Project agents", "Second workspace", "First workspace"]);
      expect(rows.map((row) => row.name)).not.toContain("other-agent");
      const firstSection = overview.overviewHtml(rows).split('aria-label="First workspace"')[1];
      expect(firstSection.indexOf("First newer")).toBeLessThan(firstSection.indexOf("First older"));
      expect(rows.find((row) => row.id === "second-agent").lastAgentMessageAt)
        .toBe(Date.parse("2026-09-22T10:00:00Z"));
    } finally {
      reader.close();
    }
  });

  it("refreshes workspace sections when cached threads and the workspace list change", async () => {
    await cache.writeCached(workspaceListAddress, [
      { id: "ws-one", project_id: "project-1", entity_id: "run-one", name: "One" },
      { id: "ws-two", project_id: "project-1", entity_id: "run-two", name: "Two" },
    ]);
    await cache.writeCached(rowAddress("run-one"), { agents: [agent("one-agent")] });
    await cache.writeCached(rowAddress("run-two"), { agents: [agent("two-agent")] });
    await cache.writeCached(workspaceThreadAddress("run-one", "one-agent"), { items: [
      datedMessage(1, "agent", "First", "2026-09-20T10:00:00Z"),
    ] });
    await cache.writeCached(workspaceThreadAddress("run-two", "two-agent"), { items: [
      datedMessage(1, "agent", "Second", "2026-09-21T10:00:00Z"),
    ] });

    const paints = [];
    const reader = overview.createAgentOverview({ scope, projectId: "project-1", includeProjectWorkspaces: true,
      sources: () => [], onRows: (rows) => paints.push(rows),
    });
    try {
      reader.open();
      await vi.waitFor(() => expect(sectionNames(paints.at(-1) || [])).toEqual(["Two", "One"]));

      await cache.writeCached(workspaceThreadAddress("run-one", "one-agent"), { items: [
        datedMessage(2, "agent", "Updated", "2026-09-22T10:00:00Z"),
      ] });
      await vi.waitFor(() => expect(sectionNames(paints.at(-1) || [])).toEqual(["One", "Two"]));

      await cache.writeCached(rowAddress("run-three"), { agents: [agent("three-agent")] });
      await cache.writeCached(workspaceListAddress, [
        { id: "ws-two", project_id: "project-1", entity_id: "run-two", name: "Two" },
        { id: "ws-three", project_id: "project-1", entity_id: "run-three", name: "Three" },
      ]);
      await vi.waitFor(() => expect(sectionNames(paints.at(-1) || [])).toEqual(["Two", "Three"]));
      expect(paints.at(-1).map((row) => row.id)).toEqual(["two-agent", "three-agent"]);
    } finally {
      reader.close();
    }
  });

  it("answers every workspace of the project, with agents or without, and keeps one whose last agent left", async () => {
    await cache.writeCached(workspaceListAddress, [
      { id: "ws-full", project_id: "project-1", entity_id: "run-full", name: "Full" },
      { id: "ws-empty", project_id: "project-1", entity_id: "run-empty", name: "Empty" },
      { id: "ws-bare", project_id: "project-1", name: "Bare" },
      { id: "ws-other", project_id: "project-2", name: "Other project" },
    ]);
    await cache.writeCached(rowAddress("run-full"), { agents: [agent("full-agent")] });
    await cache.writeCached(rowAddress("run-empty"), { agents: [] });

    const paints = [];
    const reader = overview.createAgentOverview({ scope, projectId: "project-1", includeProjectWorkspaces: true,
      sources: () => [], onRows: (rows, shape) => paints.push({ rows, shape }),
    });
    const workspaceNames = () => (paints.at(-1)?.shape?.workspaces || []).map((workspace) => workspace.name);
    try {
      reader.open();
      await vi.waitFor(() => expect(workspaceNames()).toEqual(["Full", "Empty", "Bare"]));
      expect(paints.at(-1).shape.workspaces.map((workspace) => workspace.workspaceId)).toEqual(["ws-full", "ws-empty", "ws-bare"]);

      await cache.writeCached(rowAddress("run-full"), { agents: [] });
      await vi.waitFor(() => expect(paints.at(-1).rows).toEqual([]));
      expect(workspaceNames()).toEqual(["Full", "Empty", "Bare"]);
    } finally {
      reader.close();
    }
  });

  it("prefers current work activity, then unread agent words, then the latest message", () => {
    const thread = { items: [message(1, "agent", "Earlier answer"),
      message(2, "user", "Please check this"), activity(3, "Running tests\nwith details"),
      message(4, "agent", "New answer")], activityDigests: [] };
    expect(overview.overviewSnippet(agent("A", { working: true, unread_count: 1 }), thread)).toBe("Running tests");
    expect(overview.overviewSnippet(agent("A", { unread_count: 1, read_through_sequence: 1 }), thread)).toBe("New answer");
    expect(overview.overviewSnippet(agent("A", { read_through_sequence: 4 }), thread)).toBe("New answer");
    expect(overview.overviewSnippet(agent("A"), { items: [message(1, "user", "My last question")] })).toBe("My last question");
  });

  it("mounts from cached roster and threads without a payload, then redraws on real cache writes", async () => {
    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [
      agent("A", { name: "Test agent", working: true }),
      agent("B", { name: "Review agent", unread_count: 1, read_through_sequence: 2 }),
    ] });
    await cache.writeCached(threadAddress("A"), { items: [activity(3, "Running tests")] });
    await cache.writeCached(threadAddress("B"), { items: [message(3, "agent", "Please review the diff")] });
    const paints = [];
    const reader = overview.createAgentOverview({
      scope,
      sources: () => [{ slot: "current", kind: "branch", entityId: "run-overview", address: rosterAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await settle();
    expect(paints.at(-1).map(({ name, snippet }) => [name, snippet])).toEqual([
      ["Test agent", "Running tests"], ["Review agent", "Please review the diff"],
    ]);

    await cache.writeCached(threadAddress("A"), { items: [activity(4, "Finishing lint")] });
    await settle();
    expect(paints.at(-1)[0].snippet).toBe("Finishing lint");

    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [agent("C", { name: "New agent" })] });
    await settle();
    expect(paints.at(-1).map((row) => row.name)).toEqual(["New agent"]);
    reader.close();
  });

  it("shows each cached agent's watching state as its roster changes", async () => {
    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [agent("A", { watched: false })] });
    const paints = [];
    const reader = overview.createAgentOverview({ scope,
      sources: () => [{ slot: "current", kind: "branch", entityId: "run-overview", address: rosterAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await vi.waitFor(() => expect(paints.at(-1)?.[0]?.watching).toBe(false));
    expect(overview.overviewHtml(paints.at(-1))).toContain("Not watching");
    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [agent("A", { watched: true })] });
    await vi.waitFor(() => expect(paints.at(-1)?.[0]?.watching).toBe(true));
    expect(overview.overviewHtml(paints.at(-1))).toContain("Watching");
    reader.close();
  });

  it("reads an issue agent's transcript through its cached execution context", async () => {
    const issueAddress = { deviceId: "dev-overview", entityId: "issue-1", kind: "issue", sub: "get" };
    const issueRow = { issue_id: "issue-1", agents: [agent("issue-agent", { name: "Issue agent", working: true })],
      execution_context: { entity_id: "run-implementation", agent_id: "worker-1", conversation_id: "conversation-1",
        agent: agent("worker-1", { working: false, unread_count: 1, read_through_sequence: 6 }) } };
    await cache.writeCached(issueAddress, issueRow);
    await cache.writeCached({ deviceId: "dev-overview", entityId: "run-implementation", kind: "thread", sub: "conversation-1" },
      { items: [message(7, "agent", "Implementation ready")] });
    const paints = [];
    const reader = overview.createAgentOverview({ scope,
      sources: () => [{ slot: "current", kind: "issue", entityId: "issue-1", address: issueAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await settle();
    expect(paints.at(-1)).toMatchObject([{ id: "issue-agent", name: "Issue agent",
      snippet: "Implementation ready", working: false, unread: true }]);

    await cache.writeCached(issueAddress, { ...issueRow, execution_context: { ...issueRow.execution_context,
      agent: agent("worker-1", { working: true, unread_count: 0 }) } });
    await cache.writeCached({ deviceId: "dev-overview", entityId: "run-implementation", kind: "thread", sub: "conversation-1" },
      { items: [activity(8, "Applying changes")] });
    await settle();
    expect(paints.at(-1)).toMatchObject([{ snippet: "Applying changes", working: true, unread: false }]);
    reader.close();
  });

  it("uses the execution agent's cache key when an issue has no conversation id", async () => {
    const issueAddress = { deviceId: "dev-overview", entityId: "issue-2", kind: "issue", sub: "get" };
    await cache.writeCached(issueAddress, { issue_id: "issue-2", agents: [agent("issue-agent")],
      execution_context: { entity_id: "run-2", agent_id: "worker-2" } });
    await cache.writeCached({ deviceId: "dev-overview", entityId: "run-2", kind: "thread", sub: "worker-2" },
      { items: [message(1, "agent", "Cached execution reply")] });
    const paints = [];
    const reader = overview.createAgentOverview({ scope,
      sources: () => [{ slot: "current", kind: "issue", entityId: "issue-2", address: issueAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await settle();
    expect(paints.at(-1)[0].snippet).toBe("Cached execution reply");
    reader.close();
  });
});

describe("overview scopes (#117)", () => {
  const row = (id, workspaceId, at, section = "workspace") => ({ id, source: "workspace", workspaceId,
    section, sectionName: workspaceId ? `Workspace ${workspaceId}` : "Project agents", name: id,
    snippet: "", lastAgentMessageAt: at, working: false, unread: false });
  const rows = () => [
    row("project-agent", "", 1, "project"),
    ...[1, 2, 3, 4].map((at) => row(`busy-${at}`, "busy", at)),
    row("quiet-1", "quiet", 9),
  ];
  const namesIn = (html, section) => [...html.split(`aria-label="${section}"`)[1].split("</section>")[0]
    .matchAll(/data-overview-agent="([^"]+)"/g)].map((match) => match[1]);

  it("caps each workspace at three by the overview's order on the project's scope, offering the rest", () => {
    const html = overview.overviewHtml(rows(), { showProjectAgents: true, scope: { kind: "project" } });
    expect(namesIn(html, "Workspace busy")).toEqual(["busy-4", "busy-3", "busy-2"]);
    expect(namesIn(html, "Workspace quiet")).toEqual(["quiet-1"]);
    expect(namesIn(html, "Project agents")).toEqual(["project-agent"]);
    expect(html.match(/rail-overview-see-all/g)).toHaveLength(1);
    expect(html).toContain('data-overview-scope="busy" aria-label="See all 4 agents in Workspace busy">See all</button>');
    expect(html).toContain('<button type="button" class="rail-overview-open" data-overview-scope="quiet"><span>Workspace quiet</span>');
  });

  it("shows one workspace whole, beside the project's agents, on a workspace scope", () => {
    const html = overview.overviewHtml(rows(), { showProjectAgents: true, scope: { kind: "workspace", workspaceId: "busy" } });
    expect(html).not.toContain("Workspace quiet");
    expect(namesIn(html, "Workspace busy")).toEqual(["busy-4", "busy-3", "busy-2", "busy-1"]);
    expect(html).not.toContain("rail-overview-see-all");
    expect(html).not.toContain("rail-overview-open");
  });

  it("heads every workspace it is told of, agents or none, with its way in and its +", () => {
    const workspaces = [{ workspaceId: "busy", name: "Workspace busy" }, { workspaceId: "idle", name: "Workspace idle" }];
    const html = overview.overviewHtml(rows(), { showProjectAgents: true, scope: { kind: "project" }, workspaces });
    expect(namesIn(html, "Workspace idle")).toEqual([]);
    expect(html).toContain('<button type="button" class="rail-overview-open" data-overview-scope="idle"><span>Workspace idle</span>');
    expect(html).toContain('data-overview-add="idle" aria-label="Add an agent to Workspace idle"');
    const scoped = overview.overviewHtml([], { showProjectAgents: false, scope: { kind: "workspace", workspaceId: "idle" }, workspaces });
    expect(scoped).toContain('aria-label="Workspace idle"');
    expect(scoped).not.toContain("Workspace busy");
    expect(scoped).not.toContain("No agents here yet.");
  });

  it("draws every row unscoped when the page has no scope to move between", () => {
    const html = overview.overviewHtml(rows(), { showProjectAgents: true });
    expect(namesIn(html, "Workspace busy")).toHaveLength(4);
    expect(html).not.toContain("rail-overview-see-all");
  });
});
