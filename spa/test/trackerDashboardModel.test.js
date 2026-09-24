import { describe, expect, it } from "vitest";
import {
  DONE_SINCE_CAP_MS, dashboardSections, doneMoveToday, doneSince, doneSinceCutoff, latestCachedAgentActivity,
} from "../src/core/trackerDashboardModel.js";

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
    expect(dashboardSections([today, old, unknown, movedAway], { detailById: details, nowMs: NOW }).done)
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

describe("Done since you left", () => {
  const HOUR = 60 * 60 * 1000;
  const GAP = 6 * HOUR;
  const at = (iso) => Date.parse(iso);
  /** The bridge's summary: `started`/`last` are this session, `ended` the one
   *  before it. */
  const session = (started, last, ended = null) => ({
    session_started_ms: started, last_activity_ms: last, previous_session_ended_ms: ended, gap_ms: GAP,
  });
  const finished = (id, iso) => issue(id, { status: "done", done_at: iso });

  it("reaches back over a 15 hour night, before and after the first action of the day", () => {
    const leftAt = at("2026-09-21T18:00:00Z");
    const walkIn = at("2026-09-22T09:00:00Z");
    // Walking in, nothing done yet: the bridge still describes yesterday.
    expect(doneSinceCutoff(session(at("2026-09-21T09:00:00Z"), leftAt), walkIn)).toBe(leftAt);
    // The first action has started today's session on the bridge.
    const acted = walkIn + 5 * 60_000;
    expect(doneSinceCutoff(session(acted, acted, leftAt), acted + HOUR)).toBe(leftAt);
  });

  it("covers a weekend: Friday 18:00 to Monday 09:00 is 63 hours", () => {
    const friday = at("2026-09-18T18:00:00Z");
    const monday = at("2026-09-21T09:00:00Z");
    expect(monday - friday).toBe(63 * HOUR);
    expect(doneSinceCutoff(session(friday - 9 * HOUR, friday), monday)).toBe(friday);
    expect(doneSinceCutoff(session(monday, monday + HOUR, friday), monday + 2 * HOUR)).toBe(friday);
  });

  it("covers a three-day weekend inside the 96 hour cap, all day long", () => {
    const friday = at("2026-09-18T18:00:00Z");
    const tuesday = at("2026-09-22T09:00:00Z");
    expect(tuesday - friday).toBe(87 * HOUR);
    expect(doneSinceCutoff(session(friday - 9 * HOUR, friday), tuesday)).toBe(friday);
    // Late on Tuesday it has been over 96 hours since Friday, but the absence
    // was 87: the list does not empty halfway through the day back.
    const evening = at("2026-09-22T20:00:00Z");
    expect(evening - friday).toBeGreaterThan(DONE_SINCE_CAP_MS);
    expect(doneSinceCutoff(session(tuesday, evening - HOUR, friday), evening)).toBe(friday);
  });

  it("starts a week's vacation from a blank slate that fills with this session's work", () => {
    const left = at("2026-09-11T18:00:00Z");
    const back = at("2026-09-21T09:00:00Z");
    // Walked in, nothing done yet: the cutoff is when this client saw it.
    expect(doneSinceCutoff(session(left - HOUR, left), back, back)).toBe(back);
    const acted = back + 60_000;
    const cutoff = doneSinceCutoff(session(acted, acted, left), acted + HOUR);
    expect(cutoff).toBe(acted);
    const duringVacation = finished("vacation", "2026-09-15T12:00:00Z");
    const thisMorning = finished("today", new Date(acted + 30 * 60_000).toISOString());
    expect(dashboardSections([duringVacation], { doneCutoffMs: cutoff }).done).toEqual([]);
    expect(dashboardSections([duringVacation, thisMorning], { doneCutoffMs: cutoff }).done.map((entry) => entry.issue.id))
      .toEqual(["today"]);
  });

  it("ends a session at exactly six hours of silence and not a millisecond before", () => {
    const earlier = at("2026-09-21T09:00:00Z");
    const last = at("2026-09-22T12:00:00Z");
    const summary = session(at("2026-09-22T08:00:00Z"), last, earlier);
    expect(doneSinceCutoff(summary, last + GAP)).toBe(last);
    expect(doneSinceCutoff(summary, last + GAP - 1)).toBe(earlier);
  });

  it("with no earlier session starts from this one, and with no activity at all from the walk-in", () => {
    const started = at("2026-09-22T08:00:00Z");
    expect(doneSinceCutoff(session(started, started + HOUR), started + 2 * HOUR)).toBe(started);
    expect(doneSinceCutoff(session(null, null), NOW, NOW - HOUR)).toBe(NOW - HOUR);
  });

  it("lists only issues in Done whose done_at is at or after the cutoff, from the list record alone", () => {
    const cutoff = at("2026-09-22T08:00:00Z");
    expect(doneSince(finished("at", "2026-09-22T08:00:00Z"), cutoff)).toBe("2026-09-22T08:00:00Z");
    expect(doneSince(finished("before", "2026-09-22T07:59:59Z"), cutoff)).toBeNull();
    expect(doneSince(issue("moved-away", { status: "in_review", done_at: "2026-09-22T09:00:00Z" }), cutoff)).toBeNull();
    expect(doneSince(issue("never", { status: "done" }), cutoff)).toBeNull();
    // No timeline was needed: the details map is empty.
    expect(dashboardSections([finished("x", "2026-09-22T09:00:00Z")], { doneCutoffMs: cutoff }).done)
      .toEqual([{ issue: finished("x", "2026-09-22T09:00:00Z"), movedAt: "2026-09-22T09:00:00Z", sha: null }]);
  });
});
