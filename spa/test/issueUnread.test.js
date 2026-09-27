// #104: an issue's unread, counted like an agent's, and where the rail wears it.
import { describe, expect, it } from "vitest";
import {
  issueUnreadCount,
  issueUnreadTally,
  unreadBubbleHtml,
  watchedIssuesUnread,
} from "../src/core/issueUnread.js";
import { comment, event, issue } from "./trackerWireFixture.js";

const agentSays = (id) => comment({ id, author: { kind: "agent", agent_id: "a1" } });
const userSays = (id) => comment({ id, author: { kind: "user" } });
const detailOf = (held, timeline) => ({ issue: held, timeline });

describe("one issue's unread", () => {
  it("is nothing on an issue nobody watches, whatever the list or the timeline says", () => {
    const unwatched = issue({ unread_count: 4 });
    expect(issueUnreadCount(unwatched, detailOf(unwatched, [agentSays("ic-02")]))).toBe(0);
  });

  it("is the list's count while no timeline is cached", () => {
    expect(issueUnreadCount(issue({ watched: true, unread_count: 3 }))).toBe(3);
  });

  it("counts the cached timeline past the read mark, leaving out the user's own, as #99's line does", () => {
    const held = issue({ watched: true, read_through: "ic-01", unread_count: 9 });
    const timeline = [agentSays("ic-01"), agentSays("ic-03"), userSays("ic-04"), event({ id: "ie-05", kind: "moved", actor: { kind: "agent", agent_id: "a1" } })];
    expect(issueUnreadCount(held, detailOf(held, timeline))).toBe(2);
  });

  it("reads the newer of the two read marks, so a read this tab made shows at once", () => {
    const listed = issue({ watched: true, read_through: "ic-01", unread_count: 2 });
    const read = { ...listed, read_through: "ic-03" };
    expect(issueUnreadCount(listed, detailOf(read, [agentSays("ic-02"), agentSays("ic-03")]))).toBe(0);
  });

  it("takes the list's count over a timeline older than the list", () => {
    const listed = issue({ watched: true, updated_at: "2026-08-21T11:00:00Z", unread_count: 5 });
    const older = { ...listed, updated_at: "2026-08-21T10:00:00Z" };
    expect(issueUnreadCount(listed, detailOf(older, [agentSays("ic-02")]))).toBe(5);
  });

  it("falls back to an older timeline where the bridge says no count", () => {
    const listed = issue({ watched: true, updated_at: "2026-08-21T11:00:00Z" });
    const older = { ...listed, updated_at: "2026-08-21T10:00:00Z" };
    expect(issueUnreadCount(listed, detailOf(older, [agentSays("ic-02")]))).toBe(1);
    expect(issueUnreadCount(listed)).toBe(0);
  });
});

describe("the bubble", () => {
  it("says the count, and nothing at all for none", () => {
    expect(unreadBubbleHtml(3)).toContain(">3<");
    expect(unreadBubbleHtml(3)).toContain("issue-unread");
    expect(unreadBubbleHtml(0)).toBe("");
  });
});

describe("a tab's count", () => {
  const issues = [
    issue({ id: "i-1", watched: true, unread_count: 2, assignee: { kind: "agent", agent_id: "a1" } }),
    issue({ id: "i-2", watched: true, unread_count: 3 }),
    issue({ id: "i-3", unread_count: 7, assignee: { kind: "agent", agent_id: "a1" } }),
  ];

  it("sums every watched issue's unread", () => {
    expect(watchedIssuesUnread(issues)).toBe(5);
  });

  it("sums only the ones a filter keeps", () => {
    expect(watchedIssuesUnread(issues, { only: (one) => one.assignee?.agent_id === "a1" })).toBe(2);
  });
});

describe("where the rail wears an issue's unread", () => {
  const project = { id: "p1", deviceId: "dev-1", projectKey: "dev-1|p1" };
  const held = (id, assignee, count) => issue({ id, watched: true, unread_count: count, assignee });
  const tally = issueUnreadTally([{
    project,
    issues: [
      held("i-agent", { kind: "agent", agent_id: "a1" }, 2),
      held("i-elsewhere", { kind: "agent", agent_id: "gone" }, 4),
      held("i-nobody", null, 1),
      held("i-user", { kind: "user" }, 8),
      held("i-project", { kind: "project_agent" }, 16),
      issue({ id: "i-unwatched", unread_count: 32 }),
    ],
    details: new Map(),
  }]);

  it("counts an issue on the workspace whose agent holds it", () => {
    expect(tally.heldBy("dev-1|p1", ["a1", "a2"])).toBe(2);
    expect(tally.heldBy("dev-1|other", ["a1"])).toBe(0);
  });

  it("counts everything no listed workspace holds on the project: nobody's, the user's, the project agent's, an unlisted agent's", () => {
    const rows = [{ projectKey: "dev-1|p1", agentIds: ["a1"] }];
    expect(tally.unheldBy(rows)("dev-1|p1")).toBe(1 + 4 + 8 + 16);
    expect(tally.unheldBy([])("dev-1|p1")).toBe(2 + 1 + 4 + 8 + 16);
  });
});
