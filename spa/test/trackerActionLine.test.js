/** @vitest-environment jsdom */
// An agent acting on a task, narrated in its own conversation as one line.
//
// It is a message like any other — it reads in sequence and counts as unread —
// and it is the agent saying what it just did, so it survives every detail
// level including the narrowest.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";
import { itemsAtDetailLevel } from "../src/core/conversationDetail.js";
import { actionHref, actionWord, taskActionLineHtml } from "../src/core/trackerActionLine.js";

const HERE = { deviceId: "dev-1", projectId: "proj-1" };

const action = (over = {}) => ({
  action: "commented",
  task_id: "task-01M2ZN6P",
  number: 14,
  title: "Activity entry for tasks in the conversation",
  ...over,
});

const acted = (over = {}, message = {}) => ({
  type: "message",
  data: {
    id: "message-9",
    sequence: 9,
    role: "agent",
    body: "",
    task_action: action(over),
    ...message,
  },
});

const paint = (items, place = HERE) => {
  document.body.innerHTML = threadHtml({ id: "conversation-3", items }, { place });
  return document.querySelector(".thread-task-action");
};

/// The same paint, with what the conversation knows about the project's agents
/// — which is how an assignee's id becomes a name a reader recognises.
const paintWith = (items, { agentLabels = {}, projectName = "" } = {}) => {
  document.body.innerHTML = threadHtml(
    { id: "conversation-3", items },
    { place: { ...HERE, projectName }, agentLabels },
  );
  return document.querySelector(".thread-task-action");
};

describe("the line", () => {
  it("reads as one sentence in the agent's voice", () => {
    const line = paint([acted()]);
    expect(line.textContent.replace(/\s+/g, " ").trim())
      // #323: the maintainer, "Commented on #111". What was done leads, then
      // the number. No title — it is the heading of the page the link opens —
      // and no actor, because this line IS the agent speaking in its own
      // conversation.
      .toBe("Commented on #14");
  });

  // The maintainer's four, plus the rest of the board's verbs. A bare token and
  // a past tense both read, because the bridge may send either.
  it("renders each action", () => {
    const said = (name) => paint([acted({ action: name })]).textContent.replace(/\s+/g, " ").trim();
    // The wire says `update`; a reader calls it an edit (#40).
    expect(said("updated")).toBe("Edited #14");
    expect(said("commented")).toBe("Commented on #14");
    // The token the bridge actually sends, underscore and all.
    expect(said("commented_on")).toBe("Commented on #14");
    expect(said("moved")).toBe("Moved #14");
    expect(said("closed")).toBe("Closed #14");
    expect(said("linked")).toBe("Linked #14");
  });

  // #323: "Moved #111 to “In Review”", the column in curly quotes, named the
  // way the board names it.
  it("says where a move went, when the action carries it", () => {
    const text = (over) => paint([acted(over)]).textContent.replace(/\s+/g, " ").trim();
    expect(text({ action: "moved", to: "in_review" })).toBe("Moved #14 to “In review”");
    expect(text({ action: "moved", to: "qa_hold" })).toBe("Moved #14 to “qa hold”");
  });

  // The maintainer, 21:19Z: "Use 'Created #X {title}'". A creation is the one
  // action whose news is the title, so it reads verb first and keeps the title.
  it("reads a creation verb first, with the title", () => {
    const text = (over) => paint([acted(over)]).textContent.replace(/\s+/g, " ").trim();
    expect(text({ action: "created", title: "Ghost rows survive" })).toBe("Created #14 Ghost rows survive");
    expect(text({ action: "create", title: "Ghost rows survive" })).toBe("Created #14 Ghost rows survive");
    expect(text({ action: "created", title: "" })).toBe("Created #14");
    expect(paint([acted({ action: "created", title: "Ghost rows survive" })]).querySelector(".thread-task-title")).not.toBeNull();
    expect(paint([acted({ action: "commented", title: "Ghost rows survive" })]).querySelector(".thread-task-title")).toBeNull();
  });

  // #59. An assignment's news is WHO got it, so it reads verb first like a
  // creation. "#52 assigned" told the reader the half they already knew.
  it("reads an assignment verb first, naming who got it", () => {
    const text = (over, options) =>
      paintWith([acted(over)], options).textContent.replace(/\s+/g, " ").trim();

    expect(text({ action: "assigned", assignee: { kind: "user" } })).toBe("Assigned #14 to You");
    expect(text({ action: "assigned", assignee: { kind: "project_agent" } }, { projectName: "Build" }))
      .toBe("Assigned #14 to Build");
    expect(
      text(
        { action: "assigned", assignee: { kind: "agent", agent_id: "agent-1" } },
        { agentLabels: { "agent-1": "tasks-spa · Rail scroll" } },
      ),
    ).toBe("Assigned #14 to tasks-spa · Rail scroll");

    // Handing it back is its own word and names nobody.
    expect(text({ action: "unassigned" })).toBe("Unassigned #14");
  });

  // An older bridge carried no assignee at all. The line says what it knows
  // and invents no target.
  it("says an assignment with no assignee as itself", () => {
    expect(paint([acted({ action: "assigned" })]).textContent.replace(/\s+/g, " ").trim())
      .toBe("Assigned #14");
  });

  // A later verb should leave a legible line, not a blank one — and never one
  // with an underscore still in it, which is the defect #40 was filed for.
  it("says an action it has never heard of as itself", () => {
    expect(actionWord("escalated")).toBe("escalated");
    expect(actionWord("hurled_at_wall")).toBe("hurled at wall");
    expect(actionWord("")).toBe("");
  });

  it("is one anchor over the whole line, not several", () => {
    const line = paint([acted()]);
    expect(line.tagName).toBe("A");
    expect(line.querySelectorAll("a")).toHaveLength(0);
  });
});

describe("where it goes", () => {
  it("opens the task on this machine", () => {
    expect(paint([acted({ comment_id: null })]).getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-01M2ZN6P");
  });

  // A comment is a place in the task, not just the task.
  it("lands on the comment when the action was one", () => {
    expect(paint([acted({ comment_id: "tc-01M2ZNVB" })]).getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-01M2ZN6P/c/tc-01M2ZNVB");
  });

  it("encodes a comment id that carries a separator", () => {
    expect(actionHref(action({ comment_id: "a/b" }), HERE))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-01M2ZN6P/c/a%2Fb");
  });

  // A conversation rendered with nowhere to stand points nowhere rather than
  // at a broken route.
  it("draws as plain text where there is no project to open", () => {
    const line = paint([acted()], { deviceId: null, projectId: null });
    expect(line.tagName).not.toBe("A");
    expect(line.textContent).toContain("#14");
  });

  // #323: the number is still the accent-coloured task link it was.
  it("keeps the number as its own styled part of the line", () => {
    expect(paint([acted()]).querySelector(".thread-task-number").textContent).toBe("#14");
  });

  it("draws nothing for an action naming no task", () => {
    expect(taskActionLineHtml(null)).toBe("");
    expect(taskActionLineHtml({ number: 14 })).toBe("");
  });
});

describe("it is a message like any other", () => {
  const items = () => [
    acted(),
    { type: "message", data: { id: "m1", sequence: 10, role: "agent", body: "and here is what I found" } },
    { type: "event", data: { sequence: 11, event: "tool_use", summary: "Read a file" } },
  ];

  // "Treated as a message from the agent, so it should show up in all
  // conversation view modes."
  it("survives all three detail levels", () => {
    for (const level of ["all", "messages", "agent"]) {
      const kept = itemsAtDetailLevel(items(), level);
      expect(kept.some((one) => one.data?.task_action)).toBe(true);
    }
  });

  // The narrowest level is exactly "what this agent did", which is what this
  // line says — so it must not depend on which fields the bridge happens to
  // set alongside it.
  it("survives Agent only even when the record also carries a from_agent", () => {
    const withSender = acted({}, { from_agent: { id: "agent-01M2OTHER" } });
    expect(itemsAtDetailLevel([withSender], "agent")).toHaveLength(1);
  });

  it("rides its sequence, so it reads in order and counts as unread", () => {
    paint([acted()]);
    expect(document.querySelector(".thread-message").getAttribute("data-sequence")).toBe("9");
  });

  it("leaves a message carrying no action completely alone", () => {
    document.body.innerHTML = threadHtml(
      { id: "conversation-3", items: [{ type: "message", data: { id: "m1", sequence: 1, role: "agent", body: "hello" } }] },
      { place: HERE },
    );
    expect(document.querySelector(".thread-task-action")).toBeNull();
    expect(document.querySelector(".thread-body").textContent).toContain("hello");
  });
});

describe("the comment it points at", () => {
  it("is a row the task page answers to by id", async () => {
    const { taskPageHtml } = await import("../src/core/trackerTaskRender.js");
    const { timelineRows } = await import("../src/core/trackerTimeline.js");
    const { task, comment } = await import("./trackerWireFixture.js");
    const rows = timelineRows([comment({ id: "tc-01M2ZNVB" })]);
    document.body.innerHTML = taskPageHtml(task(), { columns: null, rows, links: [], draft: "", labelsDraft: "" });
    expect(document.querySelector("#comment-tc-01M2ZNVB")).not.toBeNull();
  });
});
