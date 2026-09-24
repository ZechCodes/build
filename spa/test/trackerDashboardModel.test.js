import { describe, expect, it, vi } from "vitest";
import {
  DONE_SINCE_CAP_MS, dashboardSections, doneMoveToday, doneSince, doneSinceCutoff, latestCachedAgentActivity, standingOf,
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
  /** The bridge's summary as the cache holds it: `started`/`last` are this
   *  session, `ended` the one before it, `bridgeNow` the bridge's clock when it
   *  answered. */
  const session = (started, last, ended = null, bridgeNow = last) => ({
    session_started_ms: started, last_activity_ms: last, previous_session_ended_ms: ended, gap_ms: GAP,
    now_ms: bridgeNow,
  });
  const cutoff = (summary) => doneSinceCutoff(summary);
  const finished = (id, iso) => issue(id, { status: "done", done_at: iso });

  it("reaches back over a 15 hour night, before and after the arrival is recorded", () => {
    const leftAt = at("2026-09-21T18:00:00Z");
    const walkIn = at("2026-09-22T09:00:00Z");
    // A list read at the walk-in, before the arrival landed: the bridge still
    // describes yesterday, and its clock says the silence is 15 hours long.
    expect(cutoff(session(at("2026-09-21T09:00:00Z"), leftAt, null, walkIn))).toBe(leftAt);
    // The arrival has started today's session on the bridge.
    expect(cutoff(session(walkIn, walkIn, leftAt, walkIn + HOUR))).toBe(leftAt);
  });

  it("covers a weekend: Friday 18:00 to Monday 09:00 is 63 hours", () => {
    const friday = at("2026-09-18T18:00:00Z");
    const monday = at("2026-09-21T09:00:00Z");
    expect(monday - friday).toBe(63 * HOUR);
    expect(cutoff(session(friday - 9 * HOUR, friday, null, monday))).toBe(friday);
    expect(cutoff(session(monday, monday + HOUR, friday, monday + 2 * HOUR))).toBe(friday);
  });

  it("covers a three-day weekend inside the 96 hour cap, all day long", () => {
    const friday = at("2026-09-18T18:00:00Z");
    const tuesday = at("2026-09-22T09:00:00Z");
    expect(tuesday - friday).toBe(87 * HOUR);
    expect(cutoff(session(friday - 9 * HOUR, friday, null, tuesday))).toBe(friday);
    // Late on Tuesday it has been over 96 hours since Friday, but the absence
    // was 87: the list does not empty halfway through the day back.
    const evening = at("2026-09-22T20:00:00Z");
    expect(evening - friday).toBeGreaterThan(DONE_SINCE_CAP_MS);
    expect(cutoff(session(tuesday, evening - HOUR, friday, evening))).toBe(friday);
  });

  it("starts a week's vacation from a blank slate that fills with this session's work", () => {
    const left = at("2026-09-11T18:00:00Z");
    const back = at("2026-09-21T09:00:00Z");
    // Walked in, the arrival not recorded yet: nothing is news.
    expect(cutoff(session(left - HOUR, left, null, back))).toBe(Infinity);
    const arrival = cutoff(session(back, back, left, back + HOUR));
    expect(arrival).toBe(back);
    const duringVacation = finished("vacation", "2026-09-15T12:00:00Z");
    const thisMorning = finished("today", new Date(back + 30 * 60_000).toISOString());
    expect(dashboardSections([duringVacation], { doneCutoffMs: arrival }).done).toEqual([]);
    expect(dashboardSections([duringVacation, thisMorning], { doneCutoffMs: arrival }).done.map((entry) => entry.issue.id))
      .toEqual(["today"]);
  });

  // The reviewer's case 3: after a vacation, enter at 09:00 and see a 10:00
  // completion; a reload at 11:00 must still show it. The arrival is the
  // bridge's, so the reload reads the same start.
  it("keeps a recorded arrival across a reload hours later", () => {
    const back = at("2026-09-21T09:00:00Z");
    // The reload reads the list again at 11:00: the same session, answered later.
    const reloaded = session(back, back + 90 * 60_000, at("2026-09-11T18:00:00Z"), back + 2 * HOUR);
    expect(cutoff(reloaded)).toBe(back);
    expect(doneSince(finished("ten", "2026-09-21T10:00:00Z"), cutoff(reloaded))).toBe("2026-09-21T10:00:00Z");
  });

  it("ends a session at exactly six hours of silence and not a millisecond before", () => {
    const earlier = at("2026-09-21T09:00:00Z");
    const last = at("2026-09-22T12:00:00Z");
    const started = at("2026-09-22T08:00:00Z");
    expect(cutoff(session(started, last, earlier, last + GAP))).toBe(last);
    expect(cutoff(session(started, last, earlier, last + GAP - 1))).toBe(earlier);
    expect(standingOf(session(started, last, earlier, last + GAP))).toBe("away");
    expect(standingOf(session(started, last, earlier, last + GAP - 1))).toBe("here");
  });

  // The reviewer's case 2: nothing reads this device's clock, so a phone
  // whose clock runs hours out sees what a laptop sees from the same records.
  it("reads the silence off the bridge's answer and never this device's clock", () => {
    const earlier = at("2026-09-21T18:00:00Z");
    const last = at("2026-09-22T12:00:00Z");
    const answered = session(at("2026-09-22T09:00:00Z"), last, earlier, last + HOUR);
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(last + 30 * HOUR);
      expect(cutoff(answered)).toBe(earlier);
    } finally {
      vi.useRealTimers();
    }
    // A record with no bridge clock infers no silence at all.
    expect(cutoff({ ...answered, now_ms: null })).toBe(earlier);
  });

  // The reviewer's case: the laptop read the list at 09:00; the user kept
  // working on the phone until 14:00, which moved nothing the laptop holds;
  // the laptop repaints at 15:00. Six hours since the laptop's snapshot is not
  // six hours of silence, and the overnight work must stay.
  it("infers no silence from time passing since a snapshot another client may have kept alive", () => {
    const yesterday18 = at("2026-09-21T18:00:00Z");
    const nine = at("2026-09-22T09:00:00Z");
    const snapshot = session(nine, nine, yesterday18, nine);
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(at("2026-09-22T15:00:00Z"));
      expect(cutoff(snapshot)).toBe(yesterday18);
    } finally {
      vi.useRealTimers();
    }
    expect(doneSince(finished("overnight", "2026-09-22T03:00:00Z"), cutoff(snapshot))).toBe("2026-09-22T03:00:00Z");
  });

  it("with no earlier session starts from this one, and with no activity at all shows nothing yet", () => {
    const started = at("2026-09-22T08:00:00Z");
    expect(cutoff(session(started, started + HOUR))).toBe(started);
    expect(cutoff(session(null, null, null, NOW))).toBe(Infinity);
    expect(doneSinceCutoff(null)).toBe(Infinity);
  });

  it("lists only issues in Done whose done_at is at or after the cutoff, from the list record alone", () => {
    const from = at("2026-09-22T08:00:00Z");
    expect(doneSince(finished("at", "2026-09-22T08:00:00Z"), from)).toBe("2026-09-22T08:00:00Z");
    expect(doneSince(finished("before", "2026-09-22T07:59:59Z"), from)).toBeNull();
    expect(doneSince(issue("moved-away", { status: "in_review", done_at: "2026-09-22T09:00:00Z" }), from)).toBeNull();
    expect(doneSince(issue("never", { status: "done" }), from)).toBeNull();
    // No timeline was needed: the details map is empty.
    expect(dashboardSections([finished("x", "2026-09-22T09:00:00Z")], { doneCutoffMs: from }).done)
      .toEqual([{ issue: finished("x", "2026-09-22T09:00:00Z"), movedAt: "2026-09-22T09:00:00Z", sha: null }]);
  });
});
