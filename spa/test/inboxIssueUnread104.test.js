// @vitest-environment jsdom
// #104: a watched issue's unread counts on the rail like an agent's — on the
// workspace whose agent holds it, and otherwise on the project's own badge.
import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { TRACKER_ISSUE, inboxRowHtml, watchedWorkspaceEntries } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { issueUnreadTally } from "../src/core/issueUnread.js";
import { issue } from "./trackerWireFixture.js";

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

const tallyOf = (current, issues) =>
  issueUnreadTally([{ project: current.projects[0], issues, details: new Map() }]);

/** The rail's rows the way core/inboxView.js builds them. */
function railRows(current, issues) {
  const tally = tallyOf(current, issues);
  const workspaceRows = watchedWorkspaceEntries(current.workspaces, current.projects, current.items, current.runs, tally);
  const agentRows = projectAgentEntries(current.projects, current.items, current.runs, tally.unheldBy(workspaceRows));
  return { workspaceRow: workspaceRows[0], agentRow: agentRows[0], rows: [...workspaceRows, ...agentRows] };
}

describe("an issue held by a workspace's agent", () => {
  it("counts on that workspace's badge, whether or not that agent is watched", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      issue({ id: "i-1", watched: true, unread_count: 3, assignee: heldBy("a1") }),
      issue({ id: "i-2", watched: true, unread_count: 4, assignee: heldBy("a2") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2 + 3 + 4);
    expect(agentRow.unreadCount).toBe(1);
    const doc = new DOMParser().parseFromString(inboxRowHtml(workspaceRow), "text/html");
    expect(doc.querySelector(".inbox-actions .inbox-unread").textContent).toBe("9");
  });

  it("marks a workspace unread that only its issues are waiting on", () => {
    const quiet = snapshot({
      workspaces: [workspace],
      items: [{ kind: "branch", project_id: "project-1", run_id: "run-1", agents: [agent("a1")] }],
    });
    expect(railRows(quiet, []).workspaceRow.state).not.toBe("unread");
    const { workspaceRow } = railRows(quiet, [issue({ watched: true, unread_count: 1, assignee: heldBy("a1") })]);
    expect(workspaceRow).toMatchObject({ unreadCount: 1, state: "unread" });
  });
});

describe("an issue no listed workspace holds", () => {
  it("counts on the project's badge: nobody's, the user's, the project agent's, and an agent's with no row", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      issue({ id: "i-1", watched: true, unread_count: 1 }),
      issue({ id: "i-2", watched: true, unread_count: 2, assignee: { kind: "user" } }),
      issue({ id: "i-3", watched: true, unread_count: 4, assignee: { kind: "project_agent" } }),
      issue({ id: "i-4", watched: true, unread_count: 8, assignee: heldBy("finished-agent") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2);
    expect(agentRow).toMatchObject({ unreadCount: 1 + 1 + 2 + 4 + 8, state: "unread" });
  });

  it("counts nothing for an issue nobody watches", () => {
    const { workspaceRow, agentRow } = railRows(view(), [
      issue({ id: "i-1", unread_count: 5 }),
      issue({ id: "i-2", unread_count: 5, assignee: heldBy("a1") }),
    ]);
    expect(workspaceRow.unreadCount).toBe(2);
    expect(agentRow.unreadCount).toBe(1);
  });
});

describe("the projects face's head", () => {
  const headBadge = (issues, folded) => {
    const current = view();
    const block = workspaceProjectBlocks(railRows(current, issues).rows, current.projects, [], null, 2 * HOUR).blocks[0];
    expect(block.projectKey).toBe(PROJECT_KEY);
    const html = projectHeadHtml(block, { folded: new Set(folded ? [block.projectKey] : []) });
    return new DOMParser().parseFromString(html, "text/html").querySelector(".inbox-unread")?.textContent || null;
  };
  const issues = [
    issue({ id: "i-1", watched: true, unread_count: 3, assignee: heldBy("a1") }),
    issue({ id: "i-2", watched: true, unread_count: 10 }),
  ];

  it("wears the project's own issues while open", () => {
    expect(headBadge(issues, false)).toBe(String(1 + 10));
  });

  it("wears every issue, the workspace's too, while folded", () => {
    expect(headBadge(issues, true)).toBe(String(1 + 10 + 2 + 3));
  });

  // A watched issue asking for the user is a row in the block too (#125). Its
  // unread is already in the badges above, so the fold does not count it twice.
  it("does not count a watched issue's own row again while folded", () => {
    const current = view();
    const { rows } = railRows(current, issues);
    const issueRow = { kind: TRACKER_ISSUE, key: "tracker_issue:i-2", projectKey: PROJECT_KEY, unreadCount: 10,
      anchorMs: HOUR, lastActivityMs: HOUR };
    const block = workspaceProjectBlocks([...rows, issueRow], current.projects, [], null, 2 * HOUR).blocks[0];
    expect(block.unreadCount).toBe(1 + 10 + 2 + 3);
  });
});
