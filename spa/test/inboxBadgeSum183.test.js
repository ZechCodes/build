// @vitest-environment jsdom
// #183: one unit for the inbox's numbers. A project's head wears everything the
// project is holding, open or folded — its agent's unread, its watched tasks'
// (held or not), every workspace row's, Recent included — and the top badge is
// the sum of the heads. A fully read Needs-you row contributes zero.
// A Done or closed task never counts, whatever the bridge sends.
import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { inboxRowHtml, watchedWorkspaceEntries } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, projectsUnreadCount, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { taskUnreadTally } from "../src/core/taskUnread.js";
import { watchedTaskEntries } from "../src/core/watchedTaskRows.js";
import { attentionGroups } from "../src/core/trackerAttentionModel.js";
import { comment, event, task, taskDetail } from "./trackerWireFixture.js";

const DEVICE = "dev-1";
const HOUR = 60 * 60 * 1000;
const NOW = 30 * HOUR;

const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
const heldBy = (agentId) => ({ kind: "agent", agent_id: agentId });

const projects = [
  { id: "build", name: "Build", entity_id: "run-build", session_started_ms: NOW - HOUR, last_activity_ms: NOW - HOUR },
  { id: "smarter", name: "smarter-dev", entity_id: "run-smarter", session_started_ms: NOW - HOUR, last_activity_ms: NOW - HOUR },
];
// Build's workspace has said nothing for two days, so it is in Recent.
const workspaces = [
  { id: "ws-old", project_id: "build", name: "Old work", status: "ready", entity_id: "run-old",
    session_started_ms: NOW - 50 * HOUR, last_activity_ms: NOW - 48 * HOUR },
  { id: "ws-sd", project_id: "smarter", name: "Bot fix", status: "ready", entity_id: "run-sd",
    session_started_ms: NOW - HOUR, last_activity_ms: NOW - HOUR },
];
const items = [
  { kind: "branch", project_id: "build", run_id: "run-build", agents: [agent("build-agent", { unread_count: 1 })] },
  { kind: "branch", project_id: "build", run_id: "run-old", agents: [agent("old-agent", { unread_count: 2 })] },
  { kind: "branch", project_id: "smarter", run_id: "run-smarter", agents: [agent("sd-agent", { unread_count: 1 })] },
  { kind: "branch", project_id: "smarter", run_id: "run-sd", agents: [agent("sd-ws-agent", { unread_count: 1 })] },
];
const feed = liveFeedSnapshot({ items, runs: [] }, { projects }, { workspaces }, DEVICE);
const buildProject = feed.projects.find((project) => project.id === "build");

// #159 is in review with nothing unread: a Needs-you row. #113 is watched,
// nobody's, with 2 unread and no row. #50 is Done and a 1.29 bridge still
// sends its unread. #60 is held by Build's workspace agent.
const needsYou = task({ id: "i-159", number: 159, watched: true, status: "in_review", unread_count: 0, updated_at: new Date(NOW - HOUR).toISOString() });
const unheld = task({ id: "i-113", number: 113, watched: true, status: "ready", unread_count: 2 });
const done = task({ id: "i-50", number: 50, watched: true, status: "done", unread_count: 803 });
const held = task({ id: "i-60", number: 60, watched: true, status: "in_progress", unread_count: 3, assignee: heldBy("old-agent") });

/** Every row the rail paints, built the way core/inboxView.js builds them. */
function railRows(tasks) {
  const sources = [{ project: buildProject, tasks, details: new Map() }];
  const tally = taskUnreadTally(sources);
  const workspaceRows = watchedWorkspaceEntries(feed.workspaces, feed.projects, feed.items, feed.runs, tally);
  return [
    ...watchedTaskEntries(sources),
    ...workspaceRows,
    ...projectAgentEntries(feed.projects, feed.items, feed.runs, tally.unheldBy(workspaceRows)),
  ];
}

const blocksOf = (rows) => {
  const { blocks, recentBlocks } = workspaceProjectBlocks(rows, feed.projects, [], null, NOW);
  return new Map([...blocks, ...recentBlocks].map((block) => [block.name, block]));
};

const headBadge = (block, folded) => {
  const html = projectHeadHtml(block, { folded: new Set(folded ? [block.projectKey] : []) });
  return Number(new DOMParser().parseFromString(html, "text/html").querySelector(".inbox-unread")?.textContent || 0);
};

const soloProject = { id: "solo", name: "Solo", deviceId: DEVICE, projectKey: `${DEVICE}/solo` };
const assignedToUser = task({ id: "i-solo", number: 159, watched: true, status: "in_review",
  assignee: { kind: "user" }, read_through: "tc-02", unread_count: 0 });

function soloBadges(one, timeline) {
  const detail = taskDetail(one, timeline);
  const sources = [{ project: soloProject, tasks: [one], details: new Map([[one.id, detail]]) }];
  const [taskRow] = watchedTaskEntries(sources);
  const tally = taskUnreadTally(sources);
  const rows = [taskRow, ...projectAgentEntries([soloProject], [], [], tally.unheldBy([]))];
  const { blocks } = workspaceProjectBlocks(rows, [soloProject]);
  return { taskRow, block: blocks[0], top: projectsUnreadCount(rows, [soloProject]) };
}

describe("a fully read task assigned to the user", () => {
  it("counts an agent-created ask until read, then keeps Needs you without a badge", () => {
    const created = event({ id: "te-03", kind: "created", actor: { kind: "agent", agent_id: "a1" }, mentions_user: true });
    const unread = { ...assignedToUser, read_through: "te-02", unread_count: 1 };
    const before = soloBadges(unread, [created]);
    expect(before.taskRow.unreadCount).toBe(1);
    expect(headBadge(before.block, false)).toBe(1);
    expect(before.top).toBe(1);

    const read = { ...unread, read_through: created.id, unread_count: 0 };
    const after = soloBadges(read, [created]);
    expect(attentionGroups([read]).needsYou).toEqual([read]);
    expect(after.taskRow).toMatchObject({ state: "unread", unreadCount: 0 });
    expect(headBadge(after.block, false)).toBe(0);
    expect(after.top).toBe(0);
  });

  it("stays in Needs you with its dot while adding zero to project and top badges", () => {
    const { taskRow, block, top } = soloBadges(assignedToUser, [
      comment({ id: "tc-02", author: { kind: "agent", agent_id: "a1" } }),
    ]);
    expect(attentionGroups([assignedToUser]).needsYou).toEqual([assignedToUser]);
    expect(taskRow).toMatchObject({ state: "unread", unreadCount: 0 });
    expect(inboxRowHtml(taskRow)).toContain("sdot-unread");
    expect(headBadge(block, false)).toBe(0);
    expect(top).toBe(0);
  });

  it("adds exactly one when a new agent comment arrives after the read mark", () => {
    const read = comment({ id: "tc-02", author: { kind: "agent", agent_id: "a1" } });
    expect(soloBadges(assignedToUser, [read]).top).toBe(0);
    const asking = { ...assignedToUser, unread_count: 1 };
    const { taskRow, block, top } = soloBadges(asking, [read,
      comment({ id: "tc-03", author: { kind: "agent", agent_id: "a1" } }),
    ]);
    expect(attentionGroups([asking]).needsYou).toEqual([asking]);
    expect(taskRow.unreadCount).toBe(1);
    expect(headBadge(block, false)).toBe(1);
    expect(top).toBe(1);
  });
});

describe("a project's head", () => {
  it("wears its agent, unheld tasks and Recent workspace rows, open or folded", () => {
    const build = blocksOf(railRows([needsYou, unheld, held])).get("Build");
    expect(build.recent.map((row) => row.kind)).toEqual(["workspace"]);
    // agent 1 + #113 2 + Recent workspace (agent 2 + #60 3).
    expect(headBadge(build, false)).toBe(1 + 2 + 2 + 3);
    expect(headBadge(build, true)).toBe(1 + 2 + 2 + 3);
  });

  it("counts a Needs-you task with unread once, as its unread, not 1 more", () => {
    const asking = { ...needsYou, unread_count: 4 };
    const build = blocksOf(railRows([asking])).get("Build");
    expect(build.unreadCount).toBe(1 + 2 + 4);
  });
});

describe("finished tasks", () => {
  it("never count, whatever unread_count a 1.29 list carries", () => {
    const closed = task({ id: "i-51", watched: true, status: "ready", state: "closed", unread_count: 9 });
    const doneHeld = { ...done, id: "i-52", assignee: heldBy("old-agent") };
    const tally = taskUnreadTally([{ project: buildProject, tasks: [done, closed, doneHeld], details: new Map() }]);
    expect(tally.unheldBy([])(buildProject.projectKey)).toBe(0);
    expect(tally.heldBy(buildProject.projectKey, ["old-agent"])).toBe(0);
    const build = blocksOf(railRows([done, closed, doneHeld])).get("Build");
    expect(build.unreadCount).toBe(1 + 2);
  });
});

describe("the top badge", () => {
  it("is the sum of every project head, Recent blocks too", () => {
    const rows = railRows([needsYou, unheld, done, held]);
    const heads = [...blocksOf(rows).values()].map((block) => headBadge(block, false));
    expect(heads).toEqual([8, 2]);
    expect(projectsUnreadCount(rows, feed.projects)).toBe(10);

    const aged = feed.projects.map((project) => ({ ...project, session_started_ms: 0, last_activity_ms: 0 }));
    const { recentBlocks } = workspaceProjectBlocks(rows, aged, [], null, NOW);
    expect(recentBlocks.length).toBe(2);
    expect(projectsUnreadCount(rows, aged)).toBe(10);
  });
});
