import { describe, expect, it } from "vitest";
import {
  ATTENTION_REASONS,
  attentionGroups,
  attentionReasonLabel,
  hasUnreadInboxComment,
  taskAttention,
  unreadAsks,
  watchedTaskReasons,
} from "../src/core/trackerAttentionModel.js";

const PROJECT = "device-1/proj-1";
const task = (id, fields = {}) => ({ id, state: "open", status: "backlog", assignee: null, ...fields });
const comment = (id, kind, body) => ({ type: "comment", id, author: { kind }, body });
const detail = (one, timeline) => ({ task: one, timeline });
const inbox = (id, fields = {}) => ({ kind: "tracker_task", projectKey: PROJECT, task_id: id, unread: 1, ...fields });

describe("a watched task's inbox reasons (#125)", () => {
  it("are the Needs you reasons, for a task the user watches", () => {
    const one = task("review", { watched: true, status: "in_review", assignee: { kind: "user" }, read_through: "te-01" });
    const cached = detail(one, [comment("tc-02", "agent", "Ready for you")]);
    expect(watchedTaskReasons(one, cached)).toEqual([
      ATTENTION_REASONS.inReview, ATTENTION_REASONS.inbox, ATTENTION_REASONS.assigned,
    ]);
  });

  it("are none for a task nobody watches, or one that is Done or closed", () => {
    expect(watchedTaskReasons(task("unwatched", { status: "in_review" }), null)).toEqual([]);
    for (const fields of [{ status: "done" }, { state: "closed", status: "in_review" }]) {
      const one = task("finished", { ...fields, watched: true, assignee: { kind: "user" } });
      expect(watchedTaskReasons(one, detail(one, [comment("tc-02", "agent", "Look")]))).toEqual([]);
    }
  });

  it("count a comment as new only while it is an agent's and past the newer read mark", () => {
    const one = task("commented", { watched: true, read_through: "te-01" });
    const timeline = [comment("tc-02", "agent", "A question")];
    expect(watchedTaskReasons(one, detail(one, timeline))).toEqual([ATTENTION_REASONS.inbox]);
    expect(watchedTaskReasons(one, detail(one, [comment("tc-02", "user", "Mine")]))).toEqual([]);
    // Read on another tab: the pushed list carries the newer mark before the
    // detail record has been read again.
    expect(watchedTaskReasons({ ...one, read_through: "te-03" }, detail(one, timeline))).toEqual([]);
    // And the detail's own mark wins when it is the newer one.
    expect(watchedTaskReasons(one, detail({ ...one, read_through: "tc-02" }, timeline))).toEqual([]);
  });

  it("wait for a cached timeline before calling anything a new comment", () => {
    expect(watchedTaskReasons(task("cold", { watched: true }), null)).toEqual([]);
  });
});

describe("Needs you", () => {
  it("recognizes In review and a user assignee without a timeline", () => {
    expect(taskAttention(task("review", { status: "in_review" })).reasons).toEqual([ATTENTION_REASONS.inReview]);
    expect(taskAttention(task("mine", { assignee: { kind: "user" } })).reasons).toEqual([ATTENTION_REASONS.assigned]);
  });

  it("does not treat an agent comment as an attention reason", () => {
    const one = task("commented");
    expect(taskAttention(one, { detail: detail(one, [comment("tc-01", "agent", "Which name should I use?")]) }).reasons).toEqual([]);
  });

  it("recognizes an unread agent comment in the cached inbox, comparing ids without their type prefixes", () => {
    const one = task("mentioned", { watched: true, read_through: "te-01" });
    const cached = detail(one, [comment("tc-02", "agent", "A change for you")]);
    expect(hasUnreadInboxComment(one, cached, inbox(one.id))).toBe(true);
    expect(taskAttention(one, { detail: cached, inboxRow: inbox(one.id) }).reasons).toEqual([ATTENTION_REASONS.inbox]);
    expect(hasUnreadInboxComment(one, detail({ ...one, read_through: "te-02" }, cached.timeline), inbox(one.id))).toBe(false);
  });

  it("does not confuse watching, a dismissed row, or an unread event with an unread comment", () => {
    const one = task("quiet", { watched: true });
    const cached = detail(one, [{ type: "event", id: "te-02", actor: { kind: "agent" } }]);
    expect(hasUnreadInboxComment(one, cached, null)).toBe(false);
    expect(hasUnreadInboxComment(one, cached, inbox(one.id))).toBe(false);
    expect(hasUnreadInboxComment(one, detail(one, [comment("tc-02", "agent", "Hello")]), inbox(one.id, { done_until_next: true }))).toBe(false);
  });

  it("waits for a cached comment before attributing an unread inbox row to a mention", () => {
    const one = task("unread");
    expect(hasUnreadInboxComment(one, null, inbox(one.id))).toBe(false);
    expect(hasUnreadInboxComment(one, null, inbox(one.id, { unread: 0 }))).toBe(false);
  });

  it("returns a display reason for each supported source", () => {
    expect(Object.values(ATTENTION_REASONS).map(attentionReasonLabel)).toEqual([
      "In review", "Mentioned you", "Assigned to you",
    ]);
  });

  it("never asks for attention on Done or closed tasks, even with an unread inbox comment or user assignee", () => {
    for (const fields of [{ status: "done" }, { state: "closed", status: "in_review" }]) {
      const one = task("finished", { ...fields, assignee: { kind: "user" } });
      const cached = detail(one, [comment("tc-02", "agent", "Please look")]);
      const attention = taskAttention(one, { detail: cached, inboxRow: inbox(one.id) });
      expect(attention.needsYou).toBe(false);
      expect(attention.reasons).toEqual([]);
      expect(attention.reason).toBeNull();
    }
  });
});

describe("task list attention groups", () => {
  it("puts assigned working agents first, Needs you second, and keeps source order within each group", () => {
    const tasks = [
      task("rest-1"),
      task("needs-1", { status: "in_review" }),
      task("working-1", { assignee: { kind: "agent", agent_id: "agent-1" } }),
      task("rest-2"),
      task("working-2", { assignee: { kind: "agent", agent_id: "agent-2" } }),
      task("needs-2", { assignee: { kind: "user" } }),
    ];
    const feed = { items: [{ projectKey: PROJECT, agents: [
      { id: "agent-1", working: true }, { id: "agent-2", working: true },
    ] }] };
    const groups = attentionGroups(tasks, { feed, projectKey: PROJECT });
    expect(groups.working.map((one) => one.id)).toEqual(["working-1", "working-2"]);
    expect(groups.needsYou.map((one) => one.id)).toEqual(["needs-1", "needs-2"]);
    expect(groups.rest.map((one) => one.id)).toEqual(["rest-1", "rest-2"]);
    expect(groups.attentionById.get("working-1").holdingAgent.agent.id).toBe("agent-1");
  });

  it("groups a task held by a known idle agent with the held ones, and leaves out unknown, user and Done holders", () => {
    const idle = task("idle", { assignee: { kind: "agent", agent_id: "agent-1" } });
    const unknown = task("unknown", { assignee: { kind: "agent", agent_id: "agent-9" } });
    const mine = task("mine", { assignee: { kind: "user" } });
    const done = task("done", { status: "done", assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = { items: [{ projectKey: PROJECT, agents: [{ id: "agent-1", working: false }] }] };
    const groups = attentionGroups([idle, unknown, mine, done], { feed, projectKey: PROJECT });
    expect(groups.working).toEqual([idle]);
    expect(groups.attentionById.get("idle").holdingAgent).toMatchObject({ agent: { id: "agent-1" }, working: false });
    expect(groups.attentionById.get("unknown").holdingAgent).toBeNull();
    expect(groups.attentionById.get("done").holdingAgent).toBeNull();
  });

  it("does not borrow a working agent from another device's identically named project", () => {
    const one = task("one", { assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = { items: [{ projectKey: "device-2/proj-1", agents: [{ id: "agent-1", working: true }] }] };
    expect(attentionGroups([one], { feed, projectKey: PROJECT }).rest).toEqual([one]);
  });

  it("matches a project-agent assignment to the cached project owner's row", () => {
    const one = task("one", { assignee: { kind: "project_agent" } });
    const feed = {
      projects: [{ projectKey: PROJECT, entity_id: "run-project" }],
      items: [
        { projectKey: PROJECT, run_id: "run-workspace", agents: [{ id: "agent-other", working: true }] },
        { projectKey: PROJECT, entity_id: "run-project", agents: [{ id: "project-agent", working: true }] },
      ],
    };
    const grouped = attentionGroups([one], { feed, projectKey: PROJECT });
    expect(grouped.working).toEqual([one]);
    expect(grouped.attentionById.get(one.id).holdingAgent.agent.id).toBe("project-agent");
  });

  it("retains every reason when a working task also needs the user's look", () => {
    const one = task("one", { status: "in_review", assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = { items: [{ projectKey: PROJECT, agents: [{ id: "agent-1", working: true }] }] };
    const grouped = attentionGroups([one], { feed, projectKey: PROJECT });
    expect(grouped.working).toEqual([one]);
    expect(grouped.needsYou).toEqual([]);
    expect(grouped.attentionById.get(one.id).reasons).toEqual([ATTENTION_REASONS.inReview]);
  });
});

describe("Needs you by the narrow rule (#144)", () => {
  const asks = (id, fields) => ({ ...comment(id, "agent", "A question"), ...fields });

  it("counts a mentioned creation until its event is read, using cached records", () => {
    const one = task("created-question", { watched: true });
    const created = { type: "event", id: "te-02", kind: "created", actor: { kind: "agent" }, mentions_user: true };
    const cached = detail(one, [created]);
    const groups = () => attentionGroups([one], {
      projectKey: PROJECT, detailById: new Map([[one.id, cached]]), askedOnly: true,
    });
    expect(unreadAsks(one, cached, true)).toEqual([created]);
    expect(watchedTaskReasons(one, cached, true)).toEqual([ATTENTION_REASONS.inbox]);
    expect(groups().needsYou).toEqual([one]);
    expect(groups().attentionById.get(one.id).reason).toBe(ATTENTION_REASONS.inbox);

    one.read_through = "te-02";
    expect(watchedTaskReasons(one, cached, true)).toEqual([]);
    expect(groups().needsYou).toEqual([]);
  });

  it("leaves the In review column out, and keeps a task assigned to the user", () => {
    const review = task("review", { status: "in_review", assignee: { kind: "agent", agent_id: "agent-astra" } });
    expect(taskAttention(review, { askedOnly: true }).reasons).toEqual([]);
    const mine = task("mine", { status: "in_review", assignee: { kind: "user" } });
    expect(taskAttention(mine, { askedOnly: true }).reasons).toEqual([ATTENTION_REASONS.assigned]);
  });

  it("counts an unread agent comment only when it mentioned or asked the user", () => {
    const one = task("watched", { watched: true, read_through: "te-01" });
    const chatter = detail(one, [comment("tc-02", "agent", "Rebased on main.")]);
    expect(watchedTaskReasons(one, chatter, true)).toEqual([]);
    expect(taskAttention(one, { detail: chatter, inboxRow: inbox(one.id), askedOnly: true }).reasons).toEqual([]);
    for (const flag of [{ mentions_user: true }, { notifies_user: true }]) {
      const asked = detail(one, [comment("tc-02", "agent", "Rebased."), asks("tc-03", flag)]);
      expect(watchedTaskReasons(one, asked, true)).toEqual([ATTENTION_REASONS.inbox]);
      expect(taskAttention(one, { detail: asked, inboxRow: inbox(one.id), askedOnly: true }).reasons)
        .toEqual([ATTENTION_REASONS.inbox]);
      expect(unreadAsks(one, asked, true)).toHaveLength(1);
    }
  });

  it("reads a question from the cached task, whatever the board's feed row still says", () => {
    // An `tasks` push caches the comment and re-reads the list; the board's
    // feed row is only re-read with the board, so it can be missing or say
    // nothing unread while the question is already in the cache.
    const one = task("asked", {
      watched: true, status: "in_review", read_through: "te-01", assignee: { kind: "agent", agent_id: "agent-astra" },
    });
    const asked = detail(one, [asks("tc-02", { notifies_user: true })]);
    for (const inboxRow of [null, inbox(one.id, { unread: 0 })]) {
      expect(taskAttention(one, { detail: asked, inboxRow, askedOnly: true }).reasons).toEqual([ATTENTION_REASONS.inbox]);
    }
    const feed = { items: [inbox(one.id, { unread: 0 })] };
    expect(attentionGroups([one], { feed, projectKey: PROJECT, detailById: new Map([[one.id, asked]]), askedOnly: true })
      .needsYou).toEqual([one]);
    // Not watched: not the user's business, and a board row from before Stop
    // watching does not say otherwise. The cached task is the watch.
    const unwatched = { ...one, watched: false };
    for (const inboxRow of [null, inbox(one.id, { unread: 0 }), inbox(one.id, { unread: 1 })]) {
      expect(taskAttention(unwatched, { detail: detail(unwatched, asked.timeline), inboxRow, askedOnly: true }).reasons)
        .toEqual([]);
    }
    expect(attentionGroups([unwatched], {
      feed, projectKey: PROJECT, detailById: new Map([[one.id, detail(unwatched, asked.timeline)]]), askedOnly: true,
    }).needsYou).toEqual([]);
  });

  it("stops counting a question once it is read", () => {
    const one = task("answered", { watched: true, read_through: "tc-03" });
    expect(watchedTaskReasons(one, detail(one, [asks("tc-03", { notifies_user: true })]), true)).toEqual([]);
  });

  it("keeps the earlier rule by default, for a bridge that cannot say which comments asked", () => {
    const one = task("older", { watched: true, status: "in_review", read_through: "te-01" });
    const chatter = detail(one, [comment("tc-02", "agent", "Rebased on main.")]);
    expect(watchedTaskReasons(one, chatter)).toEqual([ATTENTION_REASONS.inReview, ATTENTION_REASONS.inbox]);
    expect(attentionGroups([one], { detailById: new Map([[one.id, chatter]]) }).needsYou).toEqual([one]);
    expect(attentionGroups([one], { detailById: new Map([[one.id, chatter]]), askedOnly: true }).needsYou).toEqual([]);
  });
});
