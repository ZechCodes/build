// @vitest-environment jsdom
// #104: a watched task's unread counts on the rail like an agent's — on the
// workspace whose agent holds it, and otherwise on the project's own badge.
import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { TRACKER_TASK, inboxRowHtml, watchedWorkspaceEntries } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { taskUnreadTally } from "../src/core/taskUnread.js";
import { task } from "./trackerWireFixture.js";

const DEVICE = "dev-1";
const PROJECT_KEY = `${DEVICE}/project-1`;
const HOUR = 60 * 60 * 1000;

const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
const heldBy = (agentId) => ({ kind: "agent", agent_id: agentId });

function snapshot({ items = [], workspaces = [] } = {}) {
  const projects = [{ id: "project-1", name: "Payments", entity_id: "run-p", session_started_ms: HOUR, last_activity_ms: HOUR }];
  return liveFeedSnapshot({ items, runs: [] }, { projects }, { workspaces }, DEVICE);
}

const workspace = { id: "ws-1", project_id: "project-1", name: "Checkout flow", status: "ready", entity_id: "run-1",
  session_started_ms: HOUR, last_activity_ms: HOUR, work_summary: { pushes: 1, behind: 0, additions: 1, deletions: 0 } };

const view = () => snapshot({
  workspaces: [workspace],
  items: [
    { kind: "branch", project_id: "project-1", run_id: "run-p", agents: [agent("project-agent", { unread_count: 1 })] },
    { kind: "branch", project_id: "project-1", run_id: "run-1", agents: [agent("a1", { unread_count: 2 }), agent("a2", { watched: false })] },
  ],
});

const tallyOf = (current, tasks) =>
  taskUnreadTally([{ project: current.projects[0], tasks, details: new Map() }]);

/** The rail's rows the way core/inboxView.js builds them. */
function railRows(current, tasks) {
  const tally = tallyOf(current, tasks);
  const workspaceRows = watchedWorkspaceEntries(current.workspaces, current.projects, current.items, current.runs, tally);
  const agentRows = projectAgentEntries(current.projects, current.items, current.runs, tally.unheldBy(workspaceRows));
  return { workspaceRow: workspaceRows[0], agentRow: agentRows[0], rows: [...workspaceRows, ...agentRows] };
}

describe("a task held by a workspace's agent", () => {
  it("counts on that workspace's badge, whether or not that agent is watched", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      task({ id: "i-1", watched: true, unread_count: 3, assignee: heldBy("a1") }),
      task({ id: "i-2", watched: true, unread_count: 4, assignee: heldBy("a2") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2 + 3 + 4);
    expect(agentRow.unreadCount).toBe(1);
    const doc = new DOMParser().parseFromString(inboxRowHtml(workspaceRow), "text/html");
    expect(doc.querySelector(".inbox-actions .inbox-unread").textContent).toBe("9");
  });

  it("marks a workspace unread that only its tasks are waiting on", () => {
    const quiet = snapshot({
      workspaces: [workspace],
      items: [{ kind: "branch", project_id: "project-1", run_id: "run-1", agents: [agent("a1")] }],
    });
    expect(railRows(quiet, []).workspaceRow.state).not.toBe("unread");
    const { workspaceRow } = railRows(quiet, [task({ watched: true, unread_count: 1, assignee: heldBy("a1") })]);
    expect(workspaceRow).toMatchObject({ unreadCount: 1, state: "unread" });
  });
});

describe("a task no listed workspace holds", () => {
  it("counts on the project's badge: nobody's, the user's, the project agent's, and an agent's with no row", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      task({ id: "i-1", watched: true, unread_count: 1 }),
      task({ id: "i-2", watched: true, unread_count: 2, assignee: { kind: "user" } }),
      task({ id: "i-3", watched: true, unread_count: 4, assignee: { kind: "project_agent" } }),
      task({ id: "i-4", watched: true, unread_count: 8, assignee: heldBy("finished-agent") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2);
    expect(agentRow).toMatchObject({ unreadCount: 1 + 1 + 2 + 4 + 8, state: "unread" });
  });

  it("counts nothing for a task nobody watches", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      task({ id: "i-1", unread_count: 5 }),
      task({ id: "i-2", unread_count: 5, assignee: heldBy("a1") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2);
    expect(agentRow.unreadCount).toBe(1);
  });
});

describe("the projects face's head", () => {
  const headBadge = (tasks, folded) => {
    const current = view();
    const block = workspaceProjectBlocks(railRows(current, tasks).rows, current.projects, [], null, 2 * HOUR).blocks[0];
    expect(block.projectKey).toBe(PROJECT_KEY);
    const html = projectHeadHtml(block, { folded: new Set(folded ? [block.projectKey] : []) });
    return new DOMParser().parseFromString(html, "text/html").querySelector(".inbox-unread")?.textContent || null;
  };
  const tasks = [
    task({ id: "i-1", watched: true, unread_count: 3, assignee: heldBy("a1") }),
    task({ id: "i-2", watched: true, unread_count: 10 }),
  ];

  // #183: the head carries everything in the block, open or folded.
  it("wears every task, the workspace's too, open or folded", () => {
    expect(headBadge(tasks, false)).toBe(String(1 + 10 + 2 + 3));
    expect(headBadge(tasks, true)).toBe(String(1 + 10 + 2 + 3));
  });

  // A watched task asking for the user is a row in the block too (#125). Its
  // unread is already in the badges above, so the head does not count it twice.
  it("does not count a watched task's own row again", () => {
    const current = view();
    const { rows } = railRows(current, tasks);
    const taskRow = { kind: TRACKER_TASK, key: "tracker_task:i-2", projectKey: PROJECT_KEY, unreadCount: 10,
      taskUnreadCount: 10, anchorMs: HOUR, lastActivityMs: HOUR };
    const block = workspaceProjectBlocks([...rows, taskRow], current.projects, [], null, 2 * HOUR).blocks[0];
    expect(block.unreadCount).toBe(1 + 10 + 2 + 3);
  });
});
