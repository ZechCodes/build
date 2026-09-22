import { describe, expect, it } from "vitest";
import {
  ATTENTION_REASONS,
  attentionGroups,
  attentionReasonLabel,
  hasUnansweredAgentQuestion,
  hasUnreadInboxComment,
  issueAttention,
} from "../src/core/trackerAttentionModel.js";

const PROJECT = "device-1/proj-1";
const issue = (id, fields = {}) => ({ id, state: "open", status: "backlog", assignee: null, ...fields });
const comment = (id, kind, body) => ({ type: "comment", id, author: { kind }, body });
const detail = (one, timeline) => ({ issue: one, timeline });
const inbox = (id, fields = {}) => ({ kind: "tracker_issue", projectKey: PROJECT, issue_id: id, unread: 1, ...fields });

describe("Needs you", () => {
  it("recognizes In review and a user assignee without a timeline", () => {
    expect(issueAttention(issue("review", { status: "in_review" })).reasons).toEqual([ATTENTION_REASONS.inReview]);
    expect(issueAttention(issue("mine", { assignee: { kind: "user" } })).reasons).toEqual([ATTENTION_REASONS.assigned]);
  });

  it("recognizes an unanswered agent question, and clears it on a later user reply", () => {
    const asked = [comment("ic-01", "agent", "Which name should I use?")];
    expect(hasUnansweredAgentQuestion(asked)).toBe(true);
    expect(hasUnansweredAgentQuestion([...asked, comment("ic-02", "user", "Use Zech")])).toBe(false);
    expect(hasUnansweredAgentQuestion([...asked, comment("ic-02", "user", "Use Zech"), comment("ic-03", "agent", "And the color?")])).toBe(true);
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
      "In review", "Asked you a question", "Mentioned you", "Assigned to you",
    ]);
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
