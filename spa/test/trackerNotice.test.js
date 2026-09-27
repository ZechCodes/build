/** @vitest-environment jsdom */
// A tracking notice as one line (#38).
//
// The maintainer, with a screenshot of a project-agent conversation: "Tracking
// notices come in looking like user messages (same color and on the right).
// They should be a single line 'X did Y on Z' deep linking."
//
// On the wire a notice IS a message on the user's side, so the two marks
// together are what tell it apart. Everything below is about the line it draws
// instead, and about the one thing that must survive a bridge that has not
// landed the structured field yet: the link.

import { describe, expect, it } from "vitest";
import { isTaskNotice, taskNoticeLineHtml, taskNoticeOf, noticeHref } from "../src/core/trackerNotice.js";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { threadHtml } from "../src/core/thread.js";
import { itemsAtDetailLevel } from "../src/core/conversationDetail.js";

const PLACE = { projectId: "proj-1", deviceId: "dev-1" };

const notice = (over = {}) => ({
  from_build: true,
  role: "user",
  from_task: { task_id: "task-32", number: 32, title: "Kanban drag does not persist" },
  body: "#32 Kanban drag does not persist — agent-01M2XXGQ commented: this reproduces on a phone too.",
  ...over,
});

const html = (message, options = { place: PLACE }) => {
  const host = document.createElement("div");
  host.innerHTML = taskNoticeLineHtml(taskNoticeOf(message), options);
  return host.firstElementChild;
};
const text = (message, options) => html(message, options).textContent.replace(/\s+/g, " ").trim();

describe("what counts as a notice", () => {
  it("is a message carrying both marks", () => {
    expect(isTaskNotice(notice())).toBe(true);
  });

  // `from_build` alone is the restart notice, which is an instruction to the
  // agent and reads as one.
  it("is not Build's own restart notice", () => {
    expect(isTaskNotice({ from_build: true, body: "Carry on." })).toBe(false);
  });

  // A hand-off carries `from_task` and is a real message from somebody.
  it("is not a task handed over in a message", () => {
    expect(isTaskNotice({ from_task: { task_id: "task-1" }, body: "Take this" })).toBe(false);
    expect(isTaskNotice(null)).toBe(false);
  });
});

describe("the line, from the structured field", () => {
  const stated = (over = {}) =>
    notice({ task_notice: { actor: "agent-01M2XXGQ", action: "commented", comment_id: "tc-9", ...over } });

  // #49. The maintainer: "Relevant info is getting pushed out of view … Move
  // what happened first and don't show the task title."
  it("reads number, action, then who did it — and no title", () => {
    expect(text(stated())).toBe("#32 commented on by Agent 01M2");
  });

  // The maintainer, 21:15Z: "What does 'moved by' mean? Moved where?" A move's
  // destination is its own field on the notice, and the line must say it.
  it("says where a move went, from the notice's own field", () => {
    expect(text(stated({ action: "moved", from: "in_progress", to: "in_review" }))).toBe("#32 moved to In review by Agent 01M2");
    expect(text(stated({ action: "moved", to: "qa_hold" }))).toBe("#32 moved to qa hold by Agent 01M2");
    expect(text(stated({ action: "moved to Done", to: "done" }))).toBe("#32 moved to Done by Agent 01M2");
  });

  it("uses the bridge's own phrase for an action it has never heard of", () => {
    expect(text(stated({ action: "moved to In review" }))).toBe("#32 moved to In review by Agent 01M2");
    expect(text(stated({ action: "assigned to Agent 2" }))).toBe("#32 assigned to Agent 2 by Agent 01M2");
  });

  // A bare token from a sender that sends one, turned into the words a reader
  // says. `comment` carries its own preposition because the sentence has none.
  it("puts a bare token into the words a reader says", () => {
    for (const [token, said] of [["comment", "commented on"], ["close", "closed"], ["reopen", "reopened"], ["edit", "edited"], ["link", "linked"], ["update", "edited"]]) {
      expect(text(stated({ action: token }))).toBe(`#32 ${said} by Agent 01M2`);
    }
  });

  it("says You for the user", () => {
    expect(text(stated({ actor: { kind: "user" } }))).toBe("#32 commented on by You");
  });

  it("uses the name this client has for an agent when it has one", () => {
    const line = taskNoticeLineHtml(taskNoticeOf(stated()), {
      place: PLACE,
      agentLabels: { "agent-01M2XXGQ": "tasks-spa · Agent 1" },
    });
    expect(line).toContain("by tasks-spa · Agent 1");
  });
});

describe("the line, parsed from the body", () => {
  it("reads the actor and the action out of the first line", () => {
    expect(text(notice())).toBe("#32 commented on by Agent 01M2");
  });

  // The comment body is carried but never drawn: the press is what opens it.
  it("never shows what was said", () => {
    expect(text(notice())).not.toContain("this reproduces on a phone too");
  });

  it("reads only the first line, however many the body has", () => {
    const long = notice({ body: `${notice().body}\n\nAnd a second paragraph.` });
    expect(text(long)).not.toContain("second paragraph");
  });

  // A title may contain an em dash; the actor never does, so the split is at
  // the last one.
  it("survives a title with an em dash in it", () => {
    const dashed = notice({
      from_task: { task_id: "task-9", number: 9, title: "Board — on a phone" },
      body: "#9 Board — on a phone — agent-01M2XXGQ closed: done.",
    });
    expect(text(dashed)).toBe("#9 closed by Agent 01M2");
  });

  // The whole point of the fallback degrading rather than failing: the task
  // comes from the envelope, so the LINK never depends on the parse.
  it("says the task alone when the prose says nothing it can read", () => {
    const opaque = notice({ body: "something else entirely" });
    expect(text(opaque)).toBe("#32");
    expect(html(opaque).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/tasks/task-32");
  });
});

describe("where the line goes", () => {
  it("opens the task's page on the machine the project is on", () => {
    expect(html(notice()).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/tasks/task-32");
  });

  it("lands on the comment itself when the field names one", () => {
    const stated = notice({ task_notice: { actor: "agent-01M2XXGQ", action: "commented", comment_id: "tc-9" } });
    expect(html(stated).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/tasks/task-32#comment-tc-9");
  });

  // Nothing in a sentence is a comment id, so a parsed notice lands on the
  // task — the right page, one scroll from the right place.
  it("lands on the task when only the prose was available", () => {
    expect(html(notice()).getAttribute("href")).not.toContain("#comment-");
  });

  // Pointing nowhere is worse than not being a link.
  it("draws as plain text where there is no project to stand in", () => {
    const loose = html(notice(), { place: null });
    expect(loose.tagName).toBe("SPAN");
    expect(noticeHref(taskNoticeOf(notice()), null)).toBe("");
  });

  it("names the task on the row, so a press can be found by id", () => {
    expect(html(notice()).dataset.taskNotice).toBe("task-32");
  });
});

// ---- the row it draws in the conversation ---------------------------------

describe("the row, in the timeline", () => {
  const item = (over = {}) => ({ type: "message", data: { id: "message-9", sequence: 9, ...notice(over) } });

  const paint = (items) => {
    document.body.innerHTML = threadHtml({ id: "conversation-3", items }, { place: PLACE });
    return document.querySelector(".thread-message");
  };

  // The whole of the maintainer's report. A notice is a message on the user's
  // side, so drawn as one it wore their colour, sat on their side, and claimed
  // they wrote it.
  it("is not a user bubble", () => {
    const row = paint([item()]);
    expect(row.classList.contains("thread-notice")).toBe(true);
    expect(row.classList.contains("user")).toBe(false);
    expect(row.classList.contains("thread-comment")).toBe(false);
    expect(row.querySelector(".thread-avatar")).toBeNull();
  });

  // Build's own restart notice is the SAME kind of row now (#42) — one quiet
  // line — but it is not about a task, so it carries no task link and its
  // press reveals the body rather than opening a page.
  it("draws Build's own notice as the same kind of row, without a task link", () => {
    const restart = { type: "message", data: { id: "m-1", sequence: 1, role: "user", from_build: true, body: "Carry on. There is more to say about it here." } };
    const row = paint([restart]);
    expect(row.classList.contains("thread-notice")).toBe(true);
    expect(row.classList.contains("thread-task-line")).toBe(true);
    expect(row.classList.contains("user")).toBe(false);
    expect(row.querySelector("[data-task-notice]")).toBeNull();
    expect(row.querySelector("details.thread-notice-more")).not.toBeNull();
  });

  it("is one line, with the comment body nowhere on it", () => {
    const row = paint([item()]);
    expect(row.querySelectorAll("a, span").length).toBeGreaterThan(0);
    expect(row.querySelector(".thread-body")).toBeNull();
    expect(row.textContent).not.toContain("this reproduces on a phone too");
  });

  // The maintainer asked for "X did Y", and X has to be a name they recognise.
  // The names come off the feed, so the timeline is handed them.
  it("names the agent the way the rest of the project names it", () => {
    document.body.innerHTML = threadHtml(
      { id: "conversation-3", items: [item({ task_notice: { actor: "agent-01M2XXGQ", action: "commented" } })] },
      { place: PLACE, agentLabels: { "agent-01M2XXGQ": "tasks-spa · Agent 1" } },
    );
    expect(document.querySelector(".thread-notice").textContent.replace(/\s+/g, " ").trim())
      .toBe("#32 commented on by tasks-spa · Agent 1");
  });

  // Not a blank where a name should be: an agent this client cannot name is
  // still said, by the four characters it wears everywhere else.
  it("falls back to the agent's short name when the feed has none for it", () => {
    expect(paint([item({ task_notice: { actor: "agent-01M2XXGQ", action: "commented" } })])
      .textContent.replace(/\s+/g, " ").trim())
      .toBe("#32 commented on by Agent 01M2");
  });

  it("carries its sequence, so it reads in order and counts as unread", () => {
    expect(paint([item()]).dataset.sequence).toBe("9");
  });

  it("does not fold", () => {
    expect(paint([item()]).querySelector("details")).toBeNull();
    expect(paint([item()]).querySelector("[data-arrival-press]")).toBeNull();
  });

  // A line nobody can see is a line nobody can press.
  it("survives every detail level", () => {
    for (const level of ["all", "messages", "agent"]) {
      expect(itemsAtDetailLevel([item()], level)).toHaveLength(1);
    }
  });

  // Explicitly, rather than falling out of carrying no `from_agent`: whether a
  // notice survives the narrowest level should not depend on which fields the
  // bridge happens to set on it.
  it("survives the narrowest level even when it names another conversation", () => {
    const relayed = { type: "message", data: { id: "m-2", sequence: 2, ...notice({ from_agent: { id: "agent-elsewhere" } }) } };
    expect(itemsAtDetailLevel([relayed], "agent")).toHaveLength(1);
  });
});

// #40. The maintainer, on the rolled build: "There's a lot of space on the left
// of the task notifications, there's a lot of space between them, and they're
// not one line." All three are structural, so all three are asserted
// structurally here — and then measured for real in a browser, which is the
// only place the first and third can actually be seen.
describe("the shape of the row", () => {
  const notice_ = (over = {}) => ({ type: "message", data: { id: "m-n", sequence: 9, ...notice(over) } });
  const action_ = (over = {}) => ({
    type: "message",
    data: {
      id: "m-a", sequence: 10, role: "agent", body: "",
      task_action: { action: "created", task_id: "task-39", number: 39, title: "A title", ...over },
    },
  });
  const said = (item) => {
    document.body.innerHTML = threadHtml({ id: "c-3", items: [item] }, { place: PLACE });
    return document.querySelector(".thread-message");
  };

  // One KIND of row, so the rules that matter are written about the kind.
  it("marks both rows as the same kind of line", () => {
    expect(said(notice_()).classList.contains("thread-task-line")).toBe(true);
    expect(said(action_()).classList.contains("thread-task-line")).toBe(true);
  });

  // The title is the part that can be any length, so it is the part that
  // gives; nothing else on the row is allowed to wrap.
  // #49: the title was the longest part of the line and the first to be cut
  // off, so it is not on the line at all any more — it is the heading of the
  // page the link opens, and it stays as hover text where length costs
  // nothing.
  it("leads with the number and carries no title on the line", () => {
    // A creation is the one exception: it reads "Created #39 A title" (the
    // maintainer, 21:19Z), covered in trackerActionLine.test.js. Every other
    // verb leads with the number.
    for (const row of [said(notice_()), said(action_({ action: "commented" }))]) {
      const line = row.querySelector("a, span");
      expect(line.firstElementChild.classList.contains("thread-task-number")).toBe(true);
      expect(line.querySelector(".thread-task-line-title")).toBeNull();
      expect(line.textContent).not.toContain("Kanban drag does not persist");
      expect(line.textContent).not.toContain("A title");
    }
  });

  it("keeps the title as hover text, so a number can still be identified", () => {
    const line = said(notice_()).querySelector("a, span");
    expect(line.getAttribute("title")).toBe("Kanban drag does not persist");
  });

  it("carries no indent of its own, so it starts where message text starts", () => {
    // The box moved to `.thread-quiet-row` in styles.css, where the tool-call
    // rows share it (#52); test/quietRows.test.js is what holds it there.
    expect(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"))
      .toContain(".thread-quiet-row { min-height:0; margin:0; padding:1px 0; }");
    // The 34 px indent the rows used to carry on top of the avatar gutter.
    expect(readFileSync(resolve(process.cwd(), "src/styles/tasks.css"), "utf8"))
      .not.toContain("padding:1px 0 1px 34px");
  });

  // Consecutive lines read as a list; a real message either side keeps the
  // full gap, because it IS a separate thing to say.
  it("pulls consecutive lines together and leaves a message alone", () => {
    const shell = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    expect(shell).toContain(
      ".thread-timeline > .thread-quiet-row + .thread-quiet-row { margin-top:calc(4px - var(--thread-gap)); }",
    );
    expect(shell).toContain("--thread-gap:18px");
    expect(shell).toContain("--thread-gap:20px");
  });

  // The actor the maintainer saw as "Agent 01M2": Build's own agent for the
  // project.
  it("names the project's agent after its project", () => {
    document.body.innerHTML = threadHtml(
      { id: "c-3", items: [notice_({ task_notice: { actor: "project-01M2SCB", action: "commented" } })] },
      { place: { ...PLACE, projectName: "Build" } },
    );
    expect(document.querySelector(".thread-notice").textContent.replace(/\s+/g, " ").trim())
      .toBe("#32 commented on by Build");
  });
});
