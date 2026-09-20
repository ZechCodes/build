/** @vitest-environment jsdom */
// #54: the picture beside a comment on the issue page says WHAT wrote it.
//
// Zech, on an issue page: "On issue comments show the harness icon as the
// profile picture."
//
// Every author wore the same grey circle with a letter in it — "A" for every
// agent that has ever commented, whichever harness it runs on and whether it
// was the project's own agent or one standing in a workspace. The letter is
// the one thing about an author that carries no information.
//
// So the mark resolves from the author: the user keeps their own, the
// project's agent wears the project's face the rail draws for it, and a
// workspace agent wears the icon of the harness it runs on — the same bundled
// artwork the rail's header draws, through the same function, because a second
// copy of a provider table is a copy that goes stale.
//
// An agent this client has never heard of keeps the generic mark. A picture is
// a claim about what wrote the comment, and guessing one is worse than not
// making it.

import { describe, expect, it } from "vitest";
import { issueAvatarHtml } from "../src/core/issueAvatar.js";
import { issuePageHtml } from "../src/core/trackerIssueRender.js";
import { agentProviders, workspaceAgents } from "../src/core/trackerAssignee.js";

const AGENTS = {
  "agent-01M2A": "claude_adk",
  "agent-01M2B": "codex",
};
const context = {
  agentLabels: { "agent-01M2A": "issues-spa · Agent 1", "agent-01M2B": "transport · Agent 1" },
  agentProviders: AGENTS,
  projectName: "Build",
};

const drawn = (actor) => {
  const host = document.createElement("div");
  host.innerHTML = issueAvatarHtml(actor, context);
  return host.firstElementChild;
};

describe("the picture beside a comment", () => {
  it("is the user's own mark for the user", () => {
    const mark = drawn({ kind: "user" });
    expect(mark.className).toBe("issue-avatar");
    expect(mark.textContent).toBe("Y");
    expect(mark.querySelector(".rail-harness-icon")).toBeNull();
  });

  // The rail squares the project's bubble off and puts the project's initial
  // in it; the page says the same thing the same way.
  it("is the project's own face for the project's agent", () => {
    const mark = drawn({ kind: "agent", agent_id: "project-01M2SCB" });
    expect(mark.classList.contains("is-project")).toBe(true);
    expect(mark.textContent).toBe("B");
  });

  // The bridge writes the project's agent as an ordinary agent actor on some
  // paths and as its own kind on others. Both are the project's agent.
  it("…however the actor was written", () => {
    expect(drawn({ kind: "project_agent" }).classList.contains("is-project")).toBe(true);
  });

  const HARNESSES = [
    { name: "Claude", actor: { kind: "agent", agent_id: "agent-01M2A" }, icon: "claude_adk" },
    { name: "Codex", actor: { kind: "agent", agent_id: "agent-01M2B" }, icon: "codex" },
  ];
  for (const { name, actor, icon } of HARNESSES) {
    it(`is the harness icon for an agent running on ${name}`, () => {
      const mark = drawn(actor);
      expect(mark.querySelector(".rail-harness-icon").dataset.harnessIcon).toBe(icon);
      expect(mark.querySelector("svg")).not.toBeNull();
      // Two harnesses, two pictures: the point of the whole issue.
      expect(mark.innerHTML).not.toBe(drawn(HARNESSES[0].actor === actor ? HARNESSES[1].actor : HARNESSES[0].actor).innerHTML);
    });
  }

  it("keeps the author's label as the hover text", () => {
    expect(drawn({ kind: "agent", agent_id: "agent-01M2A" }).title).toBe("issues-spa · Agent 1");
  });

  // An agent from a workspace this client has not read, or one that has since
  // gone: named by four characters of its id everywhere else, and marked the
  // way it always was here.
  it("is the generic mark for an agent this client does not know", () => {
    const mark = drawn({ kind: "agent", agent_id: "agent-NOPE" });
    expect(mark.className).toBe("issue-avatar");
    expect(mark.textContent).toBe("A");
    expect(mark.querySelector(".rail-harness-icon")).toBeNull();
  });
});

describe("the comment card", () => {
  const rows = [
    { type: "comment", key: "ic-1", actor: { kind: "agent", agent_id: "agent-01M2A" }, body: "Rebased.", at: "2026-09-20T10:00:00Z" },
    { type: "comment", key: "ic-2", actor: { kind: "user" }, body: "Thanks.", at: "2026-09-20T10:01:00Z" },
  ];
  const page = () => {
    const host = document.createElement("div");
    host.innerHTML = issuePageHtml(
      { id: "issue-1", number: 54, title: "An issue", state: "open", status: "in_progress", labels: [], links: {} },
      { ...context, columns: [], rows, links: [], draft: "", labelsDraft: "", busy: false, sending: false },
    );
    return host;
  };

  it("carries the picture where the letter was — same place, same size", () => {
    const [first] = [...page().querySelectorAll(".issue-comment")];
    const mark = first.firstElementChild;
    expect(mark.classList.contains("issue-avatar")).toBe(true);
    expect(mark.querySelector(".rail-harness-icon")).not.toBeNull();
  });

  it("draws each author its own", () => {
    const marks = [...page().querySelectorAll(".issue-comment > .issue-avatar")];
    expect(marks).toHaveLength(2);
    expect(marks[0].innerHTML).not.toBe(marks[1].innerHTML);
  });
});

// The providers come off the same workspace agents the assignee picker reads,
// so an agent is drawn by the same record it is named by — one read, one
// source, and no third place for the two to disagree in.
describe("where the providers come from", () => {
  const feed = {
    workspaces: [{ id: "ws-1", workspace_id: "ws-1", projectKey: "dev-1/proj-1", name: "issues-spa", entity_id: "run-1" }],
    items: [{ projectKey: "dev-1/proj-1", entity_id: "run-1", agents: [{ id: "agent-01M2A", ordinal: 1, provider: "claude_adk" }] }],
  };

  it("is the picker's own list of this project's agents", () => {
    expect(agentProviders(workspaceAgents(feed, "dev-1/proj-1"))).toEqual({ "agent-01M2A": "claude_adk" });
  });

  it("leaves out an agent whose record names no harness", () => {
    const nameless = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-01M2A", ordinal: 1 }] }] };
    expect(agentProviders(workspaceAgents(nameless, "dev-1/proj-1"))).toEqual({});
  });
});
