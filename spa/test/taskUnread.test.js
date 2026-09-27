// #104: a task's unread, counted like an agent's, and where the rail wears it.
import { describe, expect, it } from "vitest";
import {
  taskUnreadCount,
  taskUnreadTally,
  unreadBubbleHtml,
  watchedTasksUnread,
} from "../src/core/taskUnread.js";
import { comment, event, task } from "./trackerWireFixture.js";

const agentSays = (id) => comment({ id, author: { kind: "agent", agent_id: "a1" } });
const userSays = (id) => comment({ id, author: { kind: "user" } });
const detailOf = (held, timeline) => ({ task: held, timeline });

describe("one task's unread", () => {
  it("is nothing on a task nobody watches, whatever the list or the timeline says", () => {
    const unwatched = task({ unread_count: 4 });
    expect(taskUnreadCount(unwatched, detailOf(unwatched, [agentSays("tc-02")]))).toBe(0);
  });

  it("is the list's count while no timeline is cached", () => {
    expect(taskUnreadCount(task({ watched: true, unread_count: 3 }))).toBe(3);
  });

  it("counts the cached timeline past the read mark, leaving out the user's own, as #99's line does", () => {
    const held = task({ watched: true, read_through: "tc-01", unread_count: 9 });
    const timeline = [agentSays("tc-01"), agentSays("tc-03"), userSays("tc-04"), event({ id: "te-05", kind: "moved", actor: { kind: "agent", agent_id: "a1" } })];
    expect(taskUnreadCount(held, detailOf(held, timeline))).toBe(2);
  });

  it("reads the newer of the two read marks, so a read this tab made shows at once", () => {
    const listed = task({ watched: true, read_through: "tc-01", unread_count: 2 });
    const read = { ...listed, read_through: "tc-03" };
    expect(taskUnreadCount(listed, detailOf(read, [agentSays("tc-02"), agentSays("tc-03")]))).toBe(0);
  });

  it("takes the list's count over a timeline older than the list", () => {
    const listed = task({ watched: true, updated_at: "2026-08-21T11:00:00Z", unread_count: 5 });
    const older = { ...listed, updated_at: "2026-08-21T10:00:00Z" };
    expect(taskUnreadCount(listed, detailOf(older, [agentSays("tc-02")]))).toBe(5);
  });

  it("falls back to an older timeline where the bridge says no count", () => {
    const listed = task({ watched: true, updated_at: "2026-08-21T11:00:00Z" });
    const older = { ...listed, updated_at: "2026-08-21T10:00:00Z" };
    expect(taskUnreadCount(listed, detailOf(older, [agentSays("tc-02")]))).toBe(1);
    expect(taskUnreadCount(listed)).toBe(0);
  });
});

describe("the bubble", () => {
  it("says the count, and nothing at all for none", () => {
    expect(unreadBubbleHtml(3)).toContain(">3<");
    expect(unreadBubbleHtml(3)).toContain("task-unread");
    expect(unreadBubbleHtml(0)).toBe("");
  });
});

describe("a tab's count", () => {
  const tasks = [
    task({ id: "i-1", watched: true, unread_count: 2, assignee: { kind: "agent", agent_id: "a1" } }),
    task({ id: "i-2", watched: true, unread_count: 3 }),
    task({ id: "i-3", unread_count: 7, assignee: { kind: "agent", agent_id: "a1" } }),
  ];

  it("sums every watched task's unread", () => {
    expect(watchedTasksUnread(tasks)).toBe(5);
  });

  it("sums only the ones a filter keeps", () => {
    expect(watchedTasksUnread(tasks, { only: (one) => one.assignee?.agent_id === "a1" })).toBe(2);
  });
});

describe("where the rail wears a task's unread", () => {
  const project = { id: "p1", deviceId: "dev-1", projectKey: "dev-1|p1" };
  const held = (id, assignee, count) => task({ id, watched: true, unread_count: count, assignee });
  const tally = taskUnreadTally([{
    project,
    tasks: [
      held("i-agent", { kind: "agent", agent_id: "a1" }, 2),
      held("i-elsewhere", { kind: "agent", agent_id: "gone" }, 4),
      held("i-nobody", null, 1),
      held("i-user", { kind: "user" }, 8),
      held("i-project", { kind: "project_agent" }, 16),
      task({ id: "i-unwatched", unread_count: 32 }),
    ],
    details: new Map(),
  }]);

  it("counts a task on the workspace whose agent holds it", () => {
    expect(tally.heldBy("dev-1|p1", ["a1", "a2"])).toBe(2);
    expect(tally.heldBy("dev-1|other", ["a1"])).toBe(0);
  });

  it("counts everything no listed workspace holds on the project: nobody's, the user's, the project agent's, an unlisted agent's", () => {
    const rows = [{ projectKey: "dev-1|p1", agentIds: ["a1"] }];
    expect(tally.unheldBy(rows)("dev-1|p1")).toBe(1 + 4 + 8 + 16);
    expect(tally.unheldBy([])("dev-1|p1")).toBe(2 + 1 + 4 + 8 + 16);
  });
});
