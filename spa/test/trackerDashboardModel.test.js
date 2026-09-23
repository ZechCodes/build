import { describe, expect, it } from "vitest";
import { dashboardSections, doneMoveToday, latestCachedAgentActivity } from "../src/core/trackerDashboardModel.js";

const PROJECT = "device-1/proj-1";
const NOW = Date.parse("2026-09-22T20:00:00Z");
const issue = (id, fields = {}) => ({ id, status: "backlog", assignee: null, ...fields });
const move = (at, to = "done") => ({ type: "event", kind: "moved", at, payload: { from: "in_review", to } });
const detail = (timeline) => ({ timeline });

describe("Dashboard cache projection", () => {
  it("takes the latest agent activity line from a cached conversation and falls back to its digest", () => {
    const window = { items: [
      { type: "event", data: { summary: "Read the source\nSecond line" } },
      { type: "message", data: { role: "agent", body: "Updating tests\nMore details" } },
      { type: "message", data: { role: "user", body: "Please hurry" } },
    ] };
    expect(latestCachedAgentActivity(window)).toBe("Updating tests");
    expect(latestCachedAgentActivity({ items: [], activityDigests: [
      { last_tool_call: { sequence: 4, summary: "Older call" } },
      { last_tool_call: { sequence: 9, summary: "Latest call\nDetails" } },
    ] })).toBe("Latest call");
    expect(latestCachedAgentActivity(null)).toBe("");
  });

  it("names assigned working agents from the project feed and uses only a supplied cached snippet", () => {
    const work = issue("work", { assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = {
      workspaces: [{ projectKey: PROJECT, entity_id: "run-1", workspace_id: "ws-1", name: "Editor" }],
      items: [{ projectKey: PROJECT, entity_id: "run-1", agents: [
        { id: "agent-1", name: "Writer", working: true, topic: "Rewrite docs" },
      ] }],
    };
    const sections = dashboardSections([work], {
      feed, projectKey: PROJECT, nowMs: NOW, activityByAgent: new Map([["agent-1", " Checking links "]]),
    });
    expect(sections.inProgress).toEqual([{ issue: work, agentName: "Editor · Writer", activity: "Checking links" }]);
    expect(dashboardSections([work], { feed, projectKey: PROJECT, nowMs: NOW }).inProgress[0].activity).toBe("");
  });

  it("uses the list's shared Needs you reasons, including issues also assigned to working agents", () => {
    const workingReview = issue("review", { status: "in_review", assignee: { kind: "agent", agent_id: "agent-1" } });
    const assigned = issue("mine", { assignee: { kind: "user" } });
    const feed = { items: [{ projectKey: PROJECT, agents: [{ id: "agent-1", working: true }] }] };
    const sections = dashboardSections([workingReview, assigned], { feed, projectKey: PROJECT, nowMs: NOW });
    expect(sections.inProgress.map((row) => row.issue.id)).toEqual(["review"]);
    expect(sections.needsYou).toEqual([
      { issue: workingReview, reasons: ["in_review"], reasonLabels: ["In review"] },
      { issue: assigned, reasons: ["assigned_to_user"], reasonLabels: ["Assigned to you"] },
    ]);
  });

  it("names the project's own working agent after the project", () => {
    const work = issue("work", { assignee: { kind: "project_agent" } });
    const feed = {
      projects: [{ projectKey: PROJECT, name: "Build", entity_id: "project-run" }],
      items: [{ projectKey: PROJECT, entity_id: "project-run", agents: [{ id: "project-agent", working: true }] }],
    };
    expect(dashboardSections([work], { feed, projectKey: PROJECT, nowMs: NOW }).inProgress[0].agentName)
      .toBe("Build");
  });

  it("includes only cached moves into Done in the last 24 hours, with the latest linked SHA", () => {
    const today = issue("today", { status: "done", links: { commits: ["abc123", "def456"] } });
    const old = issue("old", { status: "done" });
    const unknown = issue("unknown", { status: "done", updated_at: "2026-09-22T19:00:00Z" });
    const movedAway = issue("moved-away", { status: "in_review" });
    const details = new Map([
      [today.id, detail([move("2026-09-20T12:00:00Z"), move("2026-09-22T19:00:00Z")])],
      [old.id, detail([move("2026-09-21T19:59:59Z")])],
      [movedAway.id, detail([move("2026-09-22T19:00:00Z"), move("2026-09-22T19:30:00Z", "in_review")])],
    ]);
    expect(dashboardSections([today, old, unknown, movedAway], { detailById: details, nowMs: NOW }).doneToday)
      .toEqual([{ issue: today, movedAt: "2026-09-22T19:00:00Z", sha: "def456" }]);
  });

  it("treats the 24 hour boundary as inclusive and rejects future or invalid event times", () => {
    const done = issue("done", { status: "done" });
    expect(doneMoveToday(done, detail([move("2026-09-21T20:00:00Z")]), NOW)).toBe("2026-09-21T20:00:00Z");
    expect(doneMoveToday(done, detail([move("2026-09-22T20:00:01Z")]), NOW)).toBeNull();
    expect(doneMoveToday(done, detail([move("invalid")]), NOW)).toBeNull();
    expect(doneMoveToday(done, null, NOW)).toBeNull();
  });
});
