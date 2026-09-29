/** @vitest-environment jsdom */
// #63.1: one name for an actor, everywhere the tracker prints one.
//
// The project's agent read "Build" on a notice line in the conversation
// (#49) and "Agent 01M2" on its own comment on the task page, because there
// were two functions: `actorLabel` in core/trackerModel.js, which every
// tracker surface called, and `actorName` in core/trackerLineWords.js, which
// #49 taught about the project. #54 then gave the comment card the project's
// FACE, so the row read "B — Agent 01M2": the picture and the name disagreeing
// about the same author, side by side.
//
// So there is one function now, and these are the surfaces that must agree.
// The wire writes the project's agent two ways — its own actor kind, and an
// ordinary agent actor whose id starts `project-` — and both are it.

import { describe, expect, it } from "vitest";
import { actorName } from "../src/core/trackerLineWords.js";
import { taskPageHtml } from "../src/core/trackerTaskRender.js";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { taskCardHtml } from "../src/core/trackerBoardRender.js";
import { assigneeHtml } from "../src/core/trackerChips.js";
import { noticeLineText } from "../src/core/trackerNotice.js";

const PROJECT = { kind: "agent", agent_id: "project-01M2SCB" };
const reading = {
  agentLabels: { "agent-01M2A": "tasks-spa · Agent 1" },
  projectName: "Build",
};
const columns = [{ id: "in_progress", name: "In progress" }];

const text = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.textContent.replace(/\s+/g, " ").trim();
};

describe("who an actor is", () => {
  const KINDS = [
    { name: "the project's own agent, written as its own kind", actor: { kind: "project_agent" }, reads: "Build" },
    { name: "the project's own agent, written as an agent id", actor: PROJECT, reads: "Build" },
    { name: "an agent of a workspace", actor: { kind: "agent", agent_id: "agent-01M2A" }, reads: "tasks-spa · Agent 1" },
    { name: "the reader", actor: { kind: "user" }, reads: "You" },
    { name: "an agent nothing can name", actor: { kind: "agent", agent_id: "agent-01K5ZABC" }, reads: "Agent 01K5" },
  ];

  for (const { name, actor, reads } of KINDS) {
    it(`${name} reads "${reads}"`, () => {
      expect(actorName(actor, reading)).toBe(reads);
    });
  }

  it("is nobody when there is no actor", () => {
    expect(actorName(null, reading)).toBe("");
  });

  // Without a project to name it after there is still a project agent, and
  // "Build" is this product's own name for it.
  it("names the project's agent after Build when the project has no name here", () => {
    expect(actorName(PROJECT, { agentLabels: {} })).toBe("Build");
  });
});

describe("the surfaces that print it", () => {
  const task = {
    id: "task-1", number: 63, title: "A task", state: "open", status: "in_progress",
    labels: [], links: {}, assignee: PROJECT, updated_at: "2026-09-21T00:00:00Z",
  };

  it("names the author of a comment on the task page", () => {
    const rows = [{ type: "comment", key: "c1", actor: PROJECT, body: "Rolled.", at: "2026-09-21T00:00:00Z" }];
    const html = taskPageHtml(task, { ...reading, columns, rows, links: [], draft: "", labelsDraft: "", busy: false, sending: false });
    expect(text(html)).toContain("Build");
    expect(text(html)).not.toContain("Agent 01M2");
  });

  it("names the actor of an event on the task page", () => {
    const rows = [{ type: "event", key: "e1", kind: "moved", actor: PROJECT, payload: { from: "ready", to: "in_progress" }, at: "2026-09-21T00:00:00Z" }];
    const html = taskPageHtml(task, { ...reading, columns, rows, links: [], draft: "", labelsDraft: "", busy: false, sending: false });
    expect(text(html)).toContain("Build");
  });

  it("names the assignee on a list row", () => {
    expect(text(taskRowHtml(task, { ...reading, columns, href: () => "#/x" }))).toContain("Build");
  });

  it("names the assignee on a board card", () => {
    expect(text(taskCardHtml(task, { ...reading, columns, href: () => "#/x" }))).toContain("Build");
  });

  it("names the assignee on the task's own rail", () => {
    expect(text(assigneeHtml(PROJECT, reading))).toBe("BBuild");
    expect(assigneeHtml(PROJECT, reading)).toContain("is-project");
  });

  // The line that started it: #49's notice, which was right all along. Both
  // surfaces now read the same actor through the same function.
  it("says the same as the notice line in the conversation", () => {
    const notice = { number: 63, title: "A task", action: "commented", actor: PROJECT };
    expect(noticeLineText(notice, reading)).toBe("Commented on #63 by Build");
  });
});
