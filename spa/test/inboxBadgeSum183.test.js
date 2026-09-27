// @vitest-environment jsdom
// #183: one unit for the inbox's numbers. A project's head wears everything the
// project is holding, open or folded — its agent's unread, its watched issues'
// (held or not), every workspace row's, Recent included, and 1 for a Needs-you
// issue with no unread of its own — and the top badge is the sum of the heads.
// A Done or closed issue never counts, whatever the bridge sends.
import { describe, expect, it } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { watchedWorkspaceEntries } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, projectsUnreadCount, workspaceProjectBlocks } from "../src/core/inboxProjects.js";
import { issueUnreadTally } from "../src/core/issueUnread.js";
import { watchedIssueEntries } from "../src/core/watchedIssueRows.js";
import { issue } from "./trackerWireFixture.js";

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
const needsYou = issue({ id: "i-159", number: 159, watched: true, status: "in_review", unread_count: 0, updated_at: new Date(NOW - HOUR).toISOString() });
const unheld = issue({ id: "i-113", number: 113, watched: true, status: "ready", unread_count: 2 });
const done = issue({ id: "i-50", number: 50, watched: true, status: "done", unread_count: 803 });
const held = issue({ id: "i-60", number: 60, watched: true, status: "in_progress", unread_count: 3, assignee: heldBy("old-agent") });

/** Every row the rail paints, built the way core/inboxView.js builds them. */
function railRows(issues) {
  const sources = [{ project: buildProject, issues, details: new Map() }];
  const tally = issueUnreadTally(sources);
  const workspaceRows = watchedWorkspaceEntries(feed.workspaces, feed.projects, feed.items, feed.runs, tally);
  return [
    ...watchedIssueEntries(sources),
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

describe("a project's head", () => {
  it("wears its agent, unheld issues, Recent workspace rows and a quiet Needs-you row, open or folded", () => {
    const build = blocksOf(railRows([needsYou, unheld, held])).get("Build");
    expect(build.recent.map((row) => row.kind)).toEqual(["workspace"]);
    // agent 1 + #113 2 + Recent workspace (agent 2 + #60 3) + #159's 1.
    expect(headBadge(build, false)).toBe(1 + 2 + 2 + 3 + 1);
    expect(headBadge(build, true)).toBe(1 + 2 + 2 + 3 + 1);
  });

  it("counts a Needs-you issue with unread once, as its unread, not 1 more", () => {
    const asking = { ...needsYou, unread_count: 4 };
    const build = blocksOf(railRows([asking])).get("Build");
    expect(build.unreadCount).toBe(1 + 2 + 4);
  });
});

describe("finished issues", () => {
  it("never count, whatever unread_count a 1.29 list carries", () => {
    const closed = issue({ id: "i-51", watched: true, status: "ready", state: "closed", unread_count: 9 });
    const doneHeld = { ...done, id: "i-52", assignee: heldBy("old-agent") };
    const tally = issueUnreadTally([{ project: buildProject, issues: [done, closed, doneHeld], details: new Map() }]);
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
    expect(heads).toEqual([9, 2]);
    expect(projectsUnreadCount(rows, feed.projects)).toBe(11);

    const aged = feed.projects.map((project) => ({ ...project, session_started_ms: 0, last_activity_ms: 0 }));
    const { recentBlocks } = workspaceProjectBlocks(rows, aged, [], null, NOW);
    expect(recentBlocks.length).toBe(2);
    expect(projectsUnreadCount(rows, aged)).toBe(11);
  });
});
