import { describe, expect, it } from "vitest";
import {
  ATTENTION_REASONS,
  attentionGroups,
  attentionReasonLabel,
  hasUnreadInboxComment,
  issueAttention,
  unreadAsks,
  watchedIssueReasons,
} from "../src/core/trackerAttentionModel.js";

const PROJECT = "device-1/proj-1";
const issue = (id, fields = {}) => ({ id, state: "open", status: "backlog", assignee: null, ...fields });
const comment = (id, kind, body) => ({ type: "comment", id, author: { kind }, body });
const detail = (one, timeline) => ({ issue: one, timeline });
const inbox = (id, fields = {}) => ({ kind: "tracker_issue", projectKey: PROJECT, issue_id: id, unread: 1, ...fields });

describe("a watched issue's inbox reasons (#125)", () => {
  it("are the Needs you reasons, for an issue the user watches", () => {
    const one = issue("review", { watched: true, status: "in_review", assignee: { kind: "user" }, read_through: "ie-01" });
    const cached = detail(one, [comment("ic-02", "agent", "Ready for you")]);
    expect(watchedIssueReasons(one, cached)).toEqual([
      ATTENTION_REASONS.inReview, ATTENTION_REASONS.inbox, ATTENTION_REASONS.assigned,
    ]);
  });

  it("are none for an issue nobody watches, or one that is Done or closed", () => {
    expect(watchedIssueReasons(issue("unwatched", { status: "in_review" }), null)).toEqual([]);
    for (const fields of [{ status: "done" }, { state: "closed", status: "in_review" }]) {
      const one = issue("finished", { ...fields, watched: true, assignee: { kind: "user" } });
      expect(watchedIssueReasons(one, detail(one, [comment("ic-02", "agent", "Look")]))).toEqual([]);
    }
  });

  it("count a comment as new only while it is an agent's and past the newer read mark", () => {
    const one = issue("commented", { watched: true, read_through: "ie-01" });
    const timeline = [comment("ic-02", "agent", "A question")];
    expect(watchedIssueReasons(one, detail(one, timeline))).toEqual([ATTENTION_REASONS.inbox]);
    expect(watchedIssueReasons(one, detail(one, [comment("ic-02", "user", "Mine")]))).toEqual([]);
    // Read on another tab: the pushed list carries the newer mark before the
    // detail record has been read again.
    expect(watchedIssueReasons({ ...one, read_through: "ie-03" }, detail(one, timeline))).toEqual([]);
    // And the detail's own mark wins when it is the newer one.
    expect(watchedIssueReasons(one, detail({ ...one, read_through: "ic-02" }, timeline))).toEqual([]);
  });

  it("wait for a cached timeline before calling anything a new comment", () => {
    expect(watchedIssueReasons(issue("cold", { watched: true }), null)).toEqual([]);
  });
});

describe("Needs you", () => {
  it("recognizes In review and a user assignee without a timeline", () => {
    expect(issueAttention(issue("review", { status: "in_review" })).reasons).toEqual([ATTENTION_REASONS.inReview]);
    expect(issueAttention(issue("mine", { assignee: { kind: "user" } })).reasons).toEqual([ATTENTION_REASONS.assigned]);
  });

  it("does not treat an agent comment as an attention reason", () => {
    const one = issue("commented");
    expect(issueAttention(one, { detail: detail(one, [comment("ic-01", "agent", "Which name should I use?")]) }).reasons).toEqual([]);
  });

  it("recognizes an unread agent comment in the cached inbox, comparing ids without their type prefixes", () => {
    const one = issue("mentioned", { watched: true, read_through: "ie-01" });
    const cached = detail(one, [comment("ic-02", "agent", "A change for you")]);
    expect(hasUnreadInboxComment(one, cached, inbox(one.id))).toBe(true);
    expect(issueAttention(one, { detail: cached, inboxRow: inbox(one.id) }).reasons).toEqual([ATTENTION_REASONS.inbox]);
    expect(hasUnreadInboxComment(one, detail({ ...one, read_through: "ie-02" }, cached.timeline), inbox(one.id))).toBe(false);
  });

  it("does not confuse watching, a dismissed row, or an unread event with an unread comment", () => {
    const one = issue("quiet", { watched: true });
    const cached = detail(one, [{ type: "event", id: "ie-02", actor: { kind: "agent" } }]);
    expect(hasUnreadInboxComment(one, cached, null)).toBe(false);
    expect(hasUnreadInboxComment(one, cached, inbox(one.id))).toBe(false);
    expect(hasUnreadInboxComment(one, detail(one, [comment("ic-02", "agent", "Hello")]), inbox(one.id, { done_until_next: true }))).toBe(false);
  });

  it("waits for a cached comment before attributing an unread inbox row to a mention", () => {
    const one = issue("unread");
    expect(hasUnreadInboxComment(one, null, inbox(one.id))).toBe(false);
    expect(hasUnreadInboxComment(one, null, inbox(one.id, { unread: 0 }))).toBe(false);
  });

  it("returns a display reason for each supported source", () => {
    expect(Object.values(ATTENTION_REASONS).map(attentionReasonLabel)).toEqual([
      "In review", "Mentioned you", "Assigned to you",
    ]);
  });

  it("never asks for attention on Done or closed issues, even with an unread inbox comment or user assignee", () => {
    for (const fields of [{ status: "done" }, { state: "closed", status: "in_review" }]) {
      const one = issue("finished", { ...fields, assignee: { kind: "user" } });
      const cached = detail(one, [comment("ic-02", "agent", "Please look")]);
      const attention = issueAttention(one, { detail: cached, inboxRow: inbox(one.id) });
      expect(attention.needsYou).toBe(false);
      expect(attention.reasons).toEqual([]);
      expect(attention.reason).toBeNull();
    }
  });
});

describe("issue list attention groups", () => {
  it("puts assigned working agents first, Needs you second, and keeps source order within each group", () => {
    const issues = [
      issue("rest-1"),
      issue("needs-1", { status: "in_review" }),
      issue("working-1", { assignee: { kind: "agent", agent_id: "agent-1" } }),
      issue("rest-2"),
      issue("working-2", { assignee: { kind: "agent", agent_id: "agent-2" } }),
      issue("needs-2", { assignee: { kind: "user" } }),
    ];
    const feed = { items: [{ projectKey: PROJECT, agents: [
      { id: "agent-1", working: true }, { id: "agent-2", working: true },
    ] }] };
    const groups = attentionGroups(issues, { feed, projectKey: PROJECT });
    expect(groups.working.map((one) => one.id)).toEqual(["working-1", "working-2"]);
    expect(groups.needsYou.map((one) => one.id)).toEqual(["needs-1", "needs-2"]);
    expect(groups.rest.map((one) => one.id)).toEqual(["rest-1", "rest-2"]);
    expect(groups.attentionById.get("working-1").workingAgent.agent.id).toBe("agent-1");
  });

  it("does not borrow a working agent from another device's identically named project", () => {
    const one = issue("one", { assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = { items: [{ projectKey: "device-2/proj-1", agents: [{ id: "agent-1", working: true }] }] };
    expect(attentionGroups([one], { feed, projectKey: PROJECT }).rest).toEqual([one]);
  });

  it("matches a project-agent assignment to the cached project owner's row", () => {
    const one = issue("one", { assignee: { kind: "project_agent" } });
    const feed = {
      projects: [{ projectKey: PROJECT, entity_id: "run-project" }],
      items: [
        { projectKey: PROJECT, run_id: "run-workspace", agents: [{ id: "agent-other", working: true }] },
        { projectKey: PROJECT, entity_id: "run-project", agents: [{ id: "project-agent", working: true }] },
      ],
    };
    const grouped = attentionGroups([one], { feed, projectKey: PROJECT });
    expect(grouped.working).toEqual([one]);
    expect(grouped.attentionById.get(one.id).workingAgent.agent.id).toBe("project-agent");
  });

  it("retains every reason when a working issue also needs the user's look", () => {
    const one = issue("one", { status: "in_review", assignee: { kind: "agent", agent_id: "agent-1" } });
    const feed = { items: [{ projectKey: PROJECT, agents: [{ id: "agent-1", working: true }] }] };
    const grouped = attentionGroups([one], { feed, projectKey: PROJECT });
    expect(grouped.working).toEqual([one]);
    expect(grouped.needsYou).toEqual([]);
    expect(grouped.attentionById.get(one.id).reasons).toEqual([ATTENTION_REASONS.inReview]);
  });
});

describe("Needs you by the narrow rule (#144)", () => {
  const asks = (id, fields) => ({ ...comment(id, "agent", "A question"), ...fields });

  it("leaves the In review column out, and keeps an issue assigned to the user", () => {
    const review = issue("review", { status: "in_review", assignee: { kind: "agent", agent_id: "agent-astra" } });
    expect(issueAttention(review, { askedOnly: true }).reasons).toEqual([]);
    const mine = issue("mine", { status: "in_review", assignee: { kind: "user" } });
    expect(issueAttention(mine, { askedOnly: true }).reasons).toEqual([ATTENTION_REASONS.assigned]);
  });

  it("counts an unread agent comment only when it mentioned or asked the user", () => {
    const one = issue("watched", { watched: true, read_through: "ie-01" });
    const chatter = detail(one, [comment("ic-02", "agent", "Rebased on main.")]);
    expect(watchedIssueReasons(one, chatter, true)).toEqual([]);
    expect(issueAttention(one, { detail: chatter, inboxRow: inbox(one.id), askedOnly: true }).reasons).toEqual([]);
    for (const flag of [{ mentions_user: true }, { notifies_user: true }]) {
      const asked = detail(one, [comment("ic-02", "agent", "Rebased."), asks("ic-03", flag)]);
      expect(watchedIssueReasons(one, asked, true)).toEqual([ATTENTION_REASONS.inbox]);
      expect(issueAttention(one, { detail: asked, inboxRow: inbox(one.id), askedOnly: true }).reasons)
        .toEqual([ATTENTION_REASONS.inbox]);
      expect(unreadAsks(one, asked, true)).toHaveLength(1);
    }
  });

  it("reads a question from the cached issue, whatever the board's feed row still says", () => {
    // An `issues` push caches the comment and re-reads the list; the board's
    // feed row is only re-read with the board, so it can be missing or say
    // nothing unread while the question is already in the cache.
    const one = issue("asked", {
      watched: true, status: "in_review", read_through: "ie-01", assignee: { kind: "agent", agent_id: "agent-astra" },
    });
    const asked = detail(one, [asks("ic-02", { notifies_user: true })]);
    for (const inboxRow of [null, inbox(one.id, { unread: 0 })]) {
      expect(issueAttention(one, { detail: asked, inboxRow, askedOnly: true }).reasons).toEqual([ATTENTION_REASONS.inbox]);
    }
    const feed = { items: [inbox(one.id, { unread: 0 })] };
    expect(attentionGroups([one], { feed, projectKey: PROJECT, detailById: new Map([[one.id, asked]]), askedOnly: true })
      .needsYou).toEqual([one]);
    // Not watched, and no feed row to say it is: not the user's business.
    const unwatched = { ...one, watched: false };
    expect(issueAttention(unwatched, { detail: detail(unwatched, asked.timeline), askedOnly: true }).reasons).toEqual([]);
  });

  it("stops counting a question once it is read", () => {
    const one = issue("answered", { watched: true, read_through: "ic-03" });
    expect(watchedIssueReasons(one, detail(one, [asks("ic-03", { notifies_user: true })]), true)).toEqual([]);
  });

  it("keeps the earlier rule by default, for a bridge that cannot say which comments asked", () => {
    const one = issue("older", { watched: true, status: "in_review", read_through: "ie-01" });
    const chatter = detail(one, [comment("ic-02", "agent", "Rebased on main.")]);
    expect(watchedIssueReasons(one, chatter)).toEqual([ATTENTION_REASONS.inReview, ATTENTION_REASONS.inbox]);
    expect(attentionGroups([one], { detailById: new Map([[one.id, chatter]]) }).needsYou).toEqual([one]);
    expect(attentionGroups([one], { detailById: new Map([[one.id, chatter]]), askedOnly: true }).needsYou).toEqual([]);
  });
});
