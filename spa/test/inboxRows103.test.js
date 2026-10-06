// @vitest-environment jsdom
// #103: what the inbox's workspace rows say about their agents, the project
// agent's own row, and the projects face's head badge.
import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import {
  PROJECT_AGENT,
  RECENT_AFTER_MS,
  activeEntryKey,
  byAnchor,
  inboxRowHtml,
  watchedWorkspaceEntries,
  workspaceIsRecent,
} from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";

const DEVICE = "dev-1";
const HOUR = 60 * 60 * 1000;

const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });

/** One device's snapshot, stamped the way the rail receives it. */
function snapshot({ items = [], runs = [], projects = [{ id: "project-1", name: "Payments" }], workspaces = [] } = {}) {
  return liveFeedSnapshot({ items, runs }, { projects }, { workspaces }, DEVICE);
}

const workspaceRowsOf = (view) => watchedWorkspaceEntries(view.workspaces, view.projects, view.items, view.runs);
const agentRowsOf = (view) => projectAgentEntries(view.projects, view.items, view.runs);

const readyWorkspace = { id: "ws-1", project_id: "project-1", name: "Checkout flow", status: "ready", entity_id: "run-1",
  can_finish: true, work_summary: { pushes: 1, behind: 0, additions: 12, deletions: 3 } };

describe("a workspace row's agents", () => {
  const workspaceWith = (agents, row = {}) => workspaceRowsOf(snapshot({
    workspaces: [readyWorkspace],
    items: [{ kind: "branch", project_id: "project-1", run_id: "run-1", unread_count: 99, agents, ...row }],
  }))[0];

  it("says how many agents are running after the git status", () => {
    const entry = workspaceWith([agent("a", { working: true }), agent("b", { working: true, watched: false }), agent("c")]);
    expect(entry.runningCount).toBe(2);
    expect(entry.facts).toBe("↑1 ↓0 +12 −3 · 2 running");
    expect(inboxRowHtml(entry)).toContain("↑1 ↓0 +12 −3 · 2 running");
  });

  it("says none are running rather than leaving the count out", () => {
    expect(workspaceWith([agent("a")]).facts).toBe("↑1 ↓0 +12 −3 · 0 running");
  });

  it("uses its cached roster for running activity when the feed row is stale", () => {
    const running = workspaceWith([agent("a", { working: true, unread_count: 2 })], { working: false });
    expect(running.working).toBe(true);
    const runningDoc = new DOMParser().parseFromString(inboxRowHtml(running), "text/html");
    expect(runningDoc.querySelector(".inbox-status-running.inbox-status-unread")).not.toBeNull();

    const stopped = workspaceWith([agent("a")], { working: true });
    expect(stopped.working).toBe(false);
    expect(new DOMParser().parseFromString(inboxRowHtml(stopped), "text/html").querySelector(".inbox-status-dot")).toBeNull();
  });

  it("records watched running activity separately from the workspace's running agents", () => {
    const entry = workspaceWith([agent("watched"), agent("unwatched", { watched: false, working: true })]);
    expect(entry).toMatchObject({ working: true, runningCount: 1 });
    expect(entry.watchedWorking).toBe(false);
    expect(new DOMParser().parseFromString(inboxRowHtml(entry), "text/html").querySelector(".inbox-status-running")).not.toBeNull();

    const watched = workspaceWith([agent("watched", { working: true }), agent("unwatched", { watched: false })]);
    expect(watched.watchedWorking).toBe(true);
  });

  it("counts unread over the watched agents only", () => {
    const entry = workspaceWith([
      agent("a", { unread_count: 2 }),
      agent("b", { unread_count: 5, watched: false }),
      agent("c", { unread_count: 1 }),
    ]);
    expect(entry.unreadCount).toBe(3);
  });

  it("keeps the row's own count and says nothing of running where no roster is known", () => {
    const entry = workspaceRowsOf(snapshot({
      workspaces: [{ ...readyWorkspace, entity_id: null }],
    }))[0];
    expect(entry.runningCount).toBeUndefined();
    expect(entry.facts).toBe("↑1 ↓0 +12 −3");
  });

  it("puts one unread status dot after Done", () => {
    const html = inboxRowHtml(workspaceWith([agent("a", { unread_count: 4 })]));
    const doc = new DOMParser().parseFromString(html, "text/html");
    const actions = doc.querySelector(".inbox-actions");
    expect([...actions.children].map((node) => node.className)).toEqual([
      "btn mini inbox-workspace-done",
      "inbox-status-dot inbox-status-unread",
    ]);
    expect(actions.querySelector(".inbox-status-dot").getAttribute("aria-label")).toContain("Unread");
    expect(doc.querySelector(".inbox-unread, .sdot")).toBeNull();
  });

  it("keeps the dot's place when the workspace offers no Done", () => {
    const html = inboxRowHtml(workspaceRowsOf(snapshot({
      workspaces: [{ ...readyWorkspace, status: "active" }],
      items: [{ kind: "branch", project_id: "project-1", run_id: "run-1", agents: [agent("a", { unread_count: 1 })] }],
    }))[0]);
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector(".inbox-actions > .inbox-status-unread")).not.toBeNull();
  });
});

describe("the project agent's row", () => {
  const project = (over = {}) => ({ id: "project-1", name: "Payments", entity_id: "run-p", ...over });
  const conversation = (over = {}) => ({ kind: "branch", project_id: "project-1", run_id: "run-p",
    agents: [agent("project-agent")], ...over });

  it("is one row per project, named by the project alone and opening its page", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation()] }));
    expect(entry).toMatchObject({
      kind: PROJECT_AGENT,
      key: `project-agent:${DEVICE}/project-1`,
      name: "Payments",
      entityId: "run-p",
      route: { name: "project", projectId: "project-1", deviceId: DEVICE },
    });
    const doc = new DOMParser().parseFromString(inboxRowHtml(entry), "text/html");
    expect(doc.querySelector(".stitle").textContent).toBe("Payments");
    expect(doc.querySelector(".inbox-facts")).toBeNull();
    expect(doc.querySelector(".inbox-tag")).toBeNull();
    expect(doc.querySelector("[data-workspace-done], [data-done], [data-menu]")).toBeNull();
  });

  it("carries the project agent's unread and running state", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation({
      unread: true, agents: [agent("project-agent", { unread_count: 3, working: true })],
    })] }));
    expect(entry).toMatchObject({ unreadCount: 3, state: "unread", working: true });
    const doc = new DOMParser().parseFromString(inboxRowHtml(entry), "text/html");
    expect(doc.querySelector(".inbox-actions > .inbox-status-unread")).not.toBeNull();
    expect(doc.querySelector(".inbox-status-running")).not.toBeNull();
  });

  it("is ordered by its own conversation's session, among the workspace rows", () => {
    const view = snapshot({
      projects: [project({ session_started_ms: 1 * HOUR, last_activity_ms: 30 * HOUR })],
      workspaces: [
        { ...readyWorkspace, id: "early", entity_id: null, session_started_ms: 10 * HOUR, last_activity_ms: 10 * HOUR },
        { ...readyWorkspace, id: "late", entity_id: null, session_started_ms: 30 * HOUR, last_activity_ms: 30 * HOUR },
      ],
      items: [conversation({ session_started_ms: 20 * HOUR, last_activity_ms: 21 * HOUR,
        anchor: new Date(0).toISOString() })],
    });
    const rows = [...workspaceRowsOf(view), ...agentRowsOf(view)].sort(byAnchor);
    expect(rows.map((row) => row.workspaceId || row.kind)).toEqual(["early", PROJECT_AGENT, "late"]);
    expect(rows[1]).toMatchObject({ anchorMs: 20 * HOUR, lastActivityMs: 21 * HOUR });
  });

  it("falls back to the row's own anchor on a bridge that sends no session", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation({
      anchor: "2026-09-20T10:00:00Z", last_activity: "2026-09-21T10:00:00Z",
    })] }));
    expect(entry.anchorMs).toBe(Date.parse("2026-09-20T10:00:00Z"));
    expect(entry.lastActivityMs).toBe(Date.parse("2026-09-21T10:00:00Z"));
  });

  it("moves to Recent after 24 hours without activity, and never leaves the inbox", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation({
      session_started_ms: 100, last_activity_ms: 200,
    })] }));
    expect(workspaceIsRecent(entry, 200 + RECENT_AFTER_MS)).toBe(false);
    expect(workspaceIsRecent(entry, 200 + RECENT_AFTER_MS + 1)).toBe(true);
  });

  it("stays in the inbox for a project nobody has talked to yet", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project({ entity_id: null, run_id: null })] }));
    expect(entry).toMatchObject({ entityId: null, unreadCount: 0, anchorMs: null, lastActivityMs: null });
    expect(workspaceIsRecent(entry, 1000 * RECENT_AFTER_MS)).toBe(false);
  });

  it("reads the freshest roster the board holds, even one it left out of the items", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], runs: [conversation({
      agents: [agent("project-agent", { unread_count: 2 })],
    })] }));
    expect(entry.unreadCount).toBe(2);
  });

  it("is the active row while the route stands on its project", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation()] }));
    expect(activeEntryKey({ name: "project", deviceId: DEVICE, projectId: "project-1" }, [entry])).toBe(entry.key);
    expect(activeEntryKey({ name: "project", deviceId: "dev-2", projectId: "project-1" }, [entry])).toBeNull();
  });

  it("paints as one quiet line in Recent, with its unread status dot", () => {
    const [entry] = agentRowsOf(snapshot({ projects: [project()], items: [conversation({
      agents: [agent("project-agent", { unread_count: 1 })],
    })] }));
    const doc = new DOMParser().parseFromString(inboxRowHtml(entry, { quiet: true }), "text/html");
    expect(doc.querySelector(".inbox-entry").classList.contains("inbox-quiet")).toBe(true);
    expect(doc.querySelector(".sdot")).toBeNull();
    expect(doc.querySelector(".inbox-actions > .inbox-status-unread")).not.toBeNull();
  });
});

describe("the projects face's head dot", () => {
  const view = () => snapshot({
    projects: [{ id: "project-1", name: "Payments", entity_id: "run-p", session_started_ms: HOUR, last_activity_ms: HOUR }],
    workspaces: [{ ...readyWorkspace, session_started_ms: HOUR, last_activity_ms: HOUR }],
    items: [
      { kind: "branch", project_id: "project-1", run_id: "run-p", agents: [agent("project-agent", { unread_count: 2 })] },
      { kind: "branch", project_id: "project-1", run_id: "run-1", agents: [
        agent("a", { unread_count: 3 }), agent("b", { unread_count: 7, watched: false }),
      ] },
    ],
  });
  const blockOf = (current) => {
    const rows = [...workspaceRowsOf(current), ...agentRowsOf(current)];
    return workspaceProjectBlocks(rows, current.projects, [], null, 2 * HOUR).blocks[0];
  };
  const headUnread = (block, folded) => {
    const html = projectHeadHtml(block, { folded: new Set(folded ? [block.projectKey] : []) });
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector(".inbox-unread")).toBeNull();
    return Boolean(doc.querySelector(".inbox-status-unread"));
  };

  it("does not list the project agent as a row: the head is its entry", () => {
    const block = blockOf(view());
    expect(block.entries.map((row) => row.kind)).toEqual(["workspace"]);
    expect(block.agentEntry.kind).toBe(PROJECT_AGENT);
  });

  it("keeps aggregate accounting while showing only the project agent when expanded", () => {
    const block = blockOf(view());
    expect(block.unreadCount).toBe(5);
    expect(headUnread(block, false)).toBe(true);
    expect(headUnread(block, true)).toBe(true);
    block.agentEntry = { ...block.agentEntry, ownUnreadCount: 0, unreadCount: 0 };
    expect(headUnread(block, false)).toBe(false);
    expect(headUnread(block, true)).toBe(true);
  });
});
