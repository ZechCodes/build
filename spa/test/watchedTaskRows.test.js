// Watched tasks as inbox rows (#125): a row only while the task needs the
// user, saying why, and gone the moment the reason is.
import { describe, expect, it } from "vitest";
import { watchedTaskEntries } from "../src/core/watchedTaskRows.js";
import { TRACKER_TASK, activeEntryKey, inboxRowHtml } from "../src/core/inbox.js";
import { comment, event, task, taskDetail } from "./trackerWireFixture.js";

const project = { id: "p1", deviceId: "dev-1", projectKey: "dev-1|p1", name: "Build" };
const watched = (over = {}) => task({ watched: true, ...over });
const source = (tasks, details = new Map()) => ({ project, tasks, details });
const rows = (tasks, details) => watchedTaskEntries([source(tasks, details)]);

describe("which watched tasks are rows", () => {
  it("lists one in review, assigned to the user, or with an unread agent comment", () => {
    const review = watched({ id: "i-review", number: 1, status: "in_review" });
    const mine = watched({ id: "i-mine", number: 2, assignee: { kind: "user" } });
    const asked = watched({ id: "i-asked", number: 3, read_through: "te-01" });
    const details = new Map([[asked.id, taskDetail(asked, [comment({ id: "tc-02", author: { kind: "agent", agent_id: "a1" } })])]]);
    expect(rows([review, mine, asked], details).map((row) => row.taskId)).toEqual(["i-review", "i-mine", "i-asked"]);
  });

  it("lists nothing that is only watched, not watched at all, Done, or closed", () => {
    expect(rows([
      watched({ id: "i-quiet" }),
      task({ id: "i-unwatched", status: "in_review" }),
      watched({ id: "i-done", status: "done", assignee: { kind: "user" } }),
      watched({ id: "i-closed", state: "closed", status: "in_review" }),
    ])).toEqual([]);
  });

  it("drops the row once the comment is read", () => {
    const asked = watched({ id: "i-asked", read_through: "te-01" });
    const timeline = [comment({ id: "tc-02", author: { kind: "agent", agent_id: "a1" } })];
    expect(rows([asked], new Map([[asked.id, taskDetail(asked, timeline)]]))).toHaveLength(1);
    const read = { ...asked, read_through: "tc-02" };
    expect(rows([read], new Map([[asked.id, taskDetail(read, timeline)]]))).toEqual([]);
  });
});

describe("which watched tasks are rows by the narrow rule (#144)", () => {
  const agent = { kind: "agent", agent_id: "a1" };
  const narrow = (tasks, details) => watchedTaskEntries([{ ...source(tasks, details), askedOnly: true }]);

  it("leaves out one only in review, and one with only the agents' own comments", () => {
    const review = watched({ id: "i-review", number: 1, status: "in_review", assignee: agent });
    const chatter = watched({ id: "i-chatter", number: 2, read_through: "te-01" });
    const details = new Map([[chatter.id, taskDetail(chatter, [comment({ id: "tc-02", author: agent })])]]);
    expect(narrow([review, chatter], details)).toEqual([]);
  });

  it("lists one assigned to the user and one an agent asked, with a bubble for all unread news", () => {
    const mine = watched({ id: "i-mine", number: 1, status: "in_review", assignee: { kind: "user" } });
    const asked = watched({ id: "i-asked", number: 2, read_through: "te-01" });
    const timeline = [
      comment({ id: "tc-02", author: agent }),
      comment({ id: "tc-03", author: agent, notifies_user: true }),
    ];
    const listed = narrow([mine, asked], new Map([[asked.id, taskDetail(asked, timeline)]]));
    expect(listed.map((row) => [row.taskId, row.facts, row.unreadCount])).toEqual([
      ["i-mine", "Assigned to you", 0],
      ["i-asked", "Mentioned you", 2],
    ]);
  });
});

describe("what a row says", () => {
  it("shows an unread assignment event in its bubble", () => {
    const one = watched({ id: "i-assigned", assignee: { kind: "user" }, read_through: "te-01" });
    const detail = taskDetail(one, [event({ id: "te-02", kind: "assigned", actor: { kind: "agent", agent_id: "a1" } })]);
    const [row] = rows([one], new Map([[one.id, detail]]));
    expect(row.unreadCount).toBe(1);
    expect(inboxRowHtml(row)).toContain('<span class="badge inbox-unread">1</span>');
  });

  it("counts a mentioned creation until read while an assignment keeps it in Needs you", () => {
    const one = watched({ id: "i-created", assignee: { kind: "user" }, read_through: "" });
    const detail = taskDetail(one, [event({ id: "te-02", kind: "created", actor: { kind: "agent", agent_id: "a1" }, mentions_user: true })]);
    const [row] = watchedTaskEntries([{ ...source([one], new Map([[one.id, detail]])), askedOnly: true }]);
    expect(row).toMatchObject({ state: "unread", unreadCount: 1 });
    expect(inboxRowHtml(row)).toContain('<span class="badge inbox-unread">1</span>');
    const read = { ...one, read_through: "te-02" };
    const [stillAsking] = watchedTaskEntries([{ ...source([read], new Map([[read.id, taskDetail(read, detail.timeline)]])), askedOnly: true }]);
    expect(stillAsking).toMatchObject({ state: "unread", unreadCount: 0 });
    expect(inboxRowHtml(stillAsking)).not.toContain('class="badge inbox-unread"');
  });

  it("is named by number and title, says every reason, and opens the task", () => {
    const one = watched({ id: "i-7", number: 7, title: "Wire 1.22", status: "in_review", assignee: { kind: "user" } });
    const [row] = rows([one]);
    expect(row).toMatchObject({
      kind: TRACKER_TASK,
      key: "tracker_task:i-7",
      name: "#7 Wire 1.22",
      project: "Build",
      projectKey: "dev-1|p1",
      deviceId: "dev-1",
      facts: "In review · Assigned to you",
      state: "unread",
      route: { name: "trackerTask", deviceId: "dev-1", projectId: "p1", taskId: "i-7" },
    });
  });

  it("counts its unread agent comments and names them as a new comment", () => {
    const one = watched({ id: "i-8", number: 8, read_through: "te-01" });
    const agent = { kind: "agent", agent_id: "a1" };
    const details = new Map([[one.id, taskDetail(one, [comment({ id: "tc-02", author: agent }), comment({ id: "tc-03", author: agent })])]]);
    const [row] = rows([one], details);
    expect(row.facts).toBe("New comment");
    expect(row.unreadCount).toBe(2);
    expect(inboxRowHtml(row)).toContain("New comment");
  });

  it("offers Stop watching and nothing else on its menu", () => {
    const [row] = rows([watched({ id: "i-9", number: 9, status: "in_review" })]);
    const menu = inboxRowHtml(row, { openMenuKey: row.key });
    expect(menu).toContain(`data-unwatch="${row.key}"`);
    expect(menu).not.toContain("data-dismiss");
    expect(menu).not.toContain("data-done=");
    expect(menu).not.toContain("data-mute");
  });

  it("is the active row while its task is open", () => {
    const [row] = rows([watched({ id: "i-9", number: 9, status: "in_review" })]);
    expect(activeEntryKey({ name: "trackerTask", deviceId: "dev-1", projectId: "p1", taskId: "i-9" }, [row])).toBe(row.key);
    expect(activeEntryKey({ name: "trackerTask", deviceId: "dev-2", projectId: "p1", taskId: "i-9" }, [row])).toBe(null);
  });

  it("sorts oldest change first, the inbox's own order", () => {
    const older = watched({ id: "i-old", status: "in_review", updated_at: "2026-09-23T10:00:00Z" });
    const newer = watched({ id: "i-new", status: "in_review", updated_at: "2026-09-23T11:00:00Z" });
    expect(rows([newer, older]).map((row) => row.taskId)).toEqual(["i-old", "i-new"]);
  });
});
