// @vitest-environment jsdom
// Agent-to-agent messages in one agent's conversation.
//
// Three shapes share the timeline and only one of them is the reader's own. The
// human's message keeps the right-hand bubble it always had; another agent's
// words arrive on the left, in green, under the conversation they came from;
// and a message this agent sent out is two lines that open onto what was said.
import { describe, expect, it } from "vitest";
import { createThreadState, threadHtml, wireThreadArrivals, wireThreadSentMessages } from "../src/core/thread.js";

/// Where the rail drawing the conversation is standing: the machine, and the
/// project whose page it is on. A workspace's routes are written from both.
const PLACE = { deviceId: "device-1", projectId: "proj-1" };

const PROJECT_SENDER = {
  id: "project-01M2TW0H188SATZA3NGGPQA5YM",
  owner: { kind: "project", id: "proj-9", name: "build" },
  topic: "Staffing the rail",
};

const WORKSPACE_SENDER = {
  id: "agent-9",
  owner: { kind: "workspace", id: "ws-3f2a91c4", name: "wire-facade" },
  topic: "Retry path",
};

const threadWith = (data, options = {}) =>
  threadHtml(
    { id: "conversation-3", items: [{ type: "message", data: { sequence: 1, ...data } }] },
    { place: PLACE, ...options },
  );

const arrived = (from_agent) =>
  threadWith({ id: "message-15", role: "user", body: "take the retry path next", from_agent });

const SENT_BODY = "the retry path double-posts\nand the rail is still waiting";

const sent = (sent_to, body = SENT_BODY, options = {}) =>
  threadWith(
    { id: "message-16", role: "agent", still_working: true, sent_to, body, created_at: "2026-09-13T10:05:02Z" },
    options,
  );

describe("a message another agent sent into this conversation", () => {
  it("arrives on the left, in a bubble of its own, with no avatar", () => {
    document.body.innerHTML = arrived(PROJECT_SENDER);

    const message = document.querySelector(".thread-message");
    expect(message.classList.contains("from-agent")).toBe(true);
    expect(message.classList.contains("user")).toBe(false);
    expect(message.querySelector(".thread-avatar")).toBeNull();
    expect(message.querySelector(".thread-body").textContent).toBe("take the retry path next");
  });

  it("names the project it came from and the conversation it was said in", () => {
    document.body.innerHTML = arrived(PROJECT_SENDER);

    const head = document.querySelector(".thread-from");
    expect(head.textContent.replace(/\s+/g, " ").trim()).toBe("build › Staffing the rail");
    expect(head.querySelector(".thread-from-owner").getAttribute("href")).toBe("#/device/device-1/project/proj-9");
    expect(head.querySelector(".thread-from-topic").getAttribute("href"))
      .toBe(`#/device/device-1/project/proj-9?agent=${PROJECT_SENDER.id}`);
  });

  it("writes a workspace sender against the project whose page the rail is on", () => {
    document.body.innerHTML = arrived(WORKSPACE_SENDER);

    const head = document.querySelector(".thread-from");
    expect(head.textContent.replace(/\s+/g, " ").trim()).toBe("wire-facade › Retry path");
    expect(head.querySelector(".thread-from-owner").getAttribute("href"))
      .toBe("#/device/device-1/project/proj-1/workspace/ws-3f2a91c4/changes");
    expect(head.querySelector(".thread-from-topic").getAttribute("href"))
      .toBe("#/device/device-1/project/proj-1/workspace/ws-3f2a91c4/changes?agent=agent-9");
  });

  it("says a conversation nobody named is untitled", () => {
    document.body.innerHTML = arrived({ ...WORKSPACE_SENDER, topic: "" });

    expect(document.querySelector(".thread-from-topic").textContent).toBe("Untitled conversation");
  });

  it("shows the sender's chip and no links for a record written before owners", () => {
    document.body.innerHTML = arrived({ id: "router-ef0a0d3f-f02d-49d6" });

    const head = document.querySelector(".thread-from");
    expect(head.querySelectorAll("a")).toHaveLength(0);
    expect(head.querySelector(".thread-from-chip").textContent).toBe("EF0A");
    expect(head.querySelector(".thread-from-chip").getAttribute("aria-label"))
      .toBe("Sent by agent router-ef0a0d3f-f02d-49d6");
  });

  it("wears the time like any other message and never the delivery mark", () => {
    document.body.innerHTML = arrived({ ...PROJECT_SENDER });
    expect(document.querySelector(".thread-status")).toBeNull();

    document.body.innerHTML = threadWith({
      role: "user",
      body: "take the retry path next",
      from_agent: PROJECT_SENDER,
      created_at: "2026-09-13T10:04:10Z",
    });
    expect(document.querySelector(".thread-message-footer time")).not.toBeNull();
    expect(document.querySelector(".thread-status")).toBeNull();
  });
});

/// What another agent sends is usually a report, and a report is long: the
/// whole of one laid into the conversation buries everything said around it.
/// So an arrival that runs past five lines is folded to five, with the press
/// that opens it underneath — and one that already fits is left alone, press
/// included, because there is nothing to open.
describe("a long arrival", () => {
  const REPORT = Array.from({ length: 12 }, (_, line) => `line ${line + 1} of the report`).join("\n");
  const SHORT = "took the retry path\nand the rail is quiet again";

  const report = (body = REPORT, options = {}) =>
    threadWith({ id: "message-15", role: "user", body, from_agent: PROJECT_SENDER }, options);

  const press = () => document.querySelector(".thread-arrival-press");
  const card = () => document.querySelector(".thread-comment-card");

  it("is folded to its first lines, with a press to open it", () => {
    document.body.innerHTML = report();

    expect(card().classList.contains("thread-arrival-folded")).toBe(true);
    expect(press().tagName).toBe("BUTTON");
    expect(press().getAttribute("aria-expanded")).toBe("false");
    expect(press().getAttribute("aria-controls")).toBe(card().id);
    // Folded, not truncated: the whole report is in the markup, and the reader
    // scrolls it the moment they open it.
    expect(document.querySelector(".thread-body").textContent).toContain("line 12 of the report");
  });

  it("leaves an arrival that already fits alone, press included", () => {
    document.body.innerHTML = report(SHORT);

    expect(card().classList.contains("thread-arrival-folded")).toBe(false);
    expect(press()).toBeNull();
  });

  it("counts what a line of the panel holds, not what the sender typed", () => {
    // One source line of five lines' worth of words still buries the
    // conversation, and a message of five short lines does not.
    document.body.innerHTML = report(`${"a fairly long sentence about the retry path. ".repeat(20)}`);
    expect(press()).not.toBeNull();

    document.body.innerHTML = report("one\ntwo\nthree\nfour\nfive");
    expect(press()).toBeNull();
  });

  it("opens and shuts on a press", () => {
    document.body.innerHTML = report();
    wireThreadArrivals(document.body, createThreadState());

    press().click();
    expect(press().getAttribute("aria-expanded")).toBe("true");
    expect(card().classList.contains("thread-arrival-folded")).toBe(false);
    expect(press().querySelector(".thread-arrival-less").textContent).toBe("Show less");

    press().click();
    expect(press().getAttribute("aria-expanded")).toBe("false");
    expect(card().classList.contains("thread-arrival-folded")).toBe(true);
  });

  it("opens on the keyboard too", () => {
    document.body.innerHTML = report();
    wireThreadArrivals(document.body, createThreadState());

    press().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));

    expect(press().getAttribute("aria-expanded")).toBe("true");
  });

  it("stays open through a repaint of the conversation", () => {
    const threadState = createThreadState({ ownerId: "conversation-3" });
    document.body.innerHTML = report();
    wireThreadArrivals(document.body, threadState);
    press().click();

    document.body.innerHTML = report(REPORT, { threadState });

    expect(press().getAttribute("aria-expanded")).toBe("true");
    expect(card().classList.contains("thread-arrival-folded")).toBe(false);
  });

  it("keeps the from line above it and the sequence on the row", () => {
    document.body.innerHTML = report();

    expect(document.querySelector(".thread-from-topic").textContent).toBe("Staffing the rail");
    expect(document.querySelector(".thread-message").dataset.sequence).toBe("1");
  });
});

/// The human's own message, exactly as it was drawn before any of this. The
/// markup is the assertion: the reader's side of the thread is untouched.
///
/// The card's empty slots are what the blank lines are — one per thing a
/// message may carry and this one does not. The issue card a hand-off draws
/// (`from_issue`) is one of them, and it is nothing at all on every message
/// that carries no envelope, which is every other message there is.
const USER_MESSAGE_HTML = `<section class="review-thread pane-col">
    <div class="thread-title"><span class="thread-title-text">Conversation <span>1</span></span></div>
    <div class="thread-items thread-timeline"><article class="thread-message thread-comment user" data-sequence="38">
    <span class="thread-avatar" aria-hidden="true">Y</span>
    <div class="thread-comment-card">
      
      
      
      
      
      <div class="thread-body markdown"><p>the retry path still double-posts</p></div>
      
      
      
      <div class="thread-message-footer"><span class="thread-status delivery-status sent" data-delivery-status="sent" role="status" aria-label="Sent to the agent">Sent</span></div>
    </div>
  </article></div>
    <div class="thread-revision-view" hidden></div>
    
    
  </section>`;

describe("the human's own message", () => {
  it("is drawn to the last character the way it always was", () => {
    const html = threadHtml({
      id: "conversation-3",
      items: [{
        type: "message",
        data: {
          id: "message-14",
          sequence: 38,
          role: "user",
          body: "the retry path still double-posts",
          delivery_status: "sent",
        },
      }],
    });

    expect(html).toBe(USER_MESSAGE_HTML);
  });
});

// A hand-off's body is the issue as prose, then the sender's note: the card
// draws the issue, so the body under it is the note alone, and the issue is
// said once. A body that does not start with the prose is drawn whole.
describe("a message that hands over an issue", () => {
  const envelope = { issue_id: "issue-21", number: 21, title: "Stamp the issue on what the user sends", body: "SPA half of the ask.\n\nWhen the user sends from the issue page." };
  const handed = (body) => threadHtml({
    id: "conversation-3",
    items: [{ type: "message", data: { id: "message-15", sequence: 39, role: "user", body, from_issue: envelope, delivery_status: "sent" } }],
  });

  it("draws the issue once, in the card, and the sender's note under it", () => {
    document.body.innerHTML = handed(`#21 ${envelope.title}\n\n${envelope.body}\n\nAhead of #10 in your queue.`);
    expect(document.querySelector(".thread-issue-title").textContent).toBe(envelope.title);
    expect(document.querySelector(".thread-body").textContent.trim()).toBe("Ahead of #10 in your queue.");
    expect(document.body.textContent.split("SPA half of the ask.").length - 1).toBe(1);
  });

  it("draws no body at all when nothing was said beyond the issue", () => {
    document.body.innerHTML = handed(`#21 ${envelope.title}\n\n${envelope.body}`);
    expect(document.querySelector(".thread-issue")).not.toBeNull();
    expect(document.querySelector(".thread-body")).toBeNull();
  });

  it("draws a body whole when it does not start with the issue's prose", () => {
    document.body.innerHTML = handed("Please take this one.");
    expect(document.querySelector(".thread-body").textContent.trim()).toBe("Please take this one.");
  });
});

describe("a message this agent sent to another agent", () => {
  it("is two lines: where it went, and the first line of what was said", () => {
    document.body.innerHTML = sent(WORKSPACE_SENDER);

    const item = document.querySelector(".thread-sent");
    expect(item.classList.contains("thread-message")).toBe(true);
    const label = item.querySelector(".thread-sent-label");
    expect(label.textContent.replace(/\s+/g, " ").trim()).toBe("Sent a message to wire-facade › Retry path");
    expect(label.querySelector(".thread-sent-owner").getAttribute("href"))
      .toBe("#/device/device-1/project/proj-1/workspace/ws-3f2a91c4/changes");
    expect(label.querySelector(".thread-sent-topic").getAttribute("href"))
      .toBe("#/device/device-1/project/proj-1/workspace/ws-3f2a91c4/changes?agent=agent-9");
    expect(item.querySelector(".thread-sent-head time")).not.toBeNull();

    const line = item.querySelector(".thread-sent-preview");
    expect(line.tagName).toBe("BUTTON");
    expect(line.querySelector(".thread-sent-first").textContent).toBe("the retry path double-posts");
    expect(line.getAttribute("aria-expanded")).toBe("false");
    expect(item.querySelector(".thread-body").hidden).toBe(true);
  });

  it("opens onto the whole of what was said when the line is pressed", () => {
    document.body.innerHTML = sent(WORKSPACE_SENDER);
    wireThreadSentMessages(document.body, createThreadState());

    document.querySelector(".thread-sent-preview").click();

    const item = document.querySelector(".thread-sent");
    expect(item.querySelector(".thread-sent-preview").getAttribute("aria-expanded")).toBe("true");
    expect(item.querySelector(".thread-body").hidden).toBe(false);
    expect(item.querySelector(".thread-body").textContent).toContain("and the rail is still waiting");
    expect(item.querySelector(".thread-sent-shut").textContent).toBe("Hide");
  });

  it("opens on the keyboard too, and shuts on a second press", () => {
    document.body.innerHTML = sent(WORKSPACE_SENDER);
    wireThreadSentMessages(document.body, createThreadState());
    const line = document.querySelector(".thread-sent-preview");

    line.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(line.getAttribute("aria-expanded")).toBe("true");

    line.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(line.getAttribute("aria-expanded")).toBe("false");
    expect(document.querySelector(".thread-body").hidden).toBe(true);
  });

  it("stays open through a repaint of the conversation", () => {
    const threadState = createThreadState({ ownerId: "conversation-3" });
    document.body.innerHTML = sent(WORKSPACE_SENDER);
    wireThreadSentMessages(document.body, threadState);
    document.querySelector(".thread-sent-preview").click();

    document.body.innerHTML = sent(WORKSPACE_SENDER, undefined, { threadState });

    expect(document.querySelector(".thread-sent-preview").getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelector(".thread-body").hidden).toBe(false);
  });

  it("is a row of its own, so the activity around it falls into two runs", () => {
    document.body.innerHTML = threadHtml({
      id: "conversation-3",
      items: [
        { type: "event", data: { sequence: 1, event: "tool_use", summary: "read post.rs" } },
        { type: "message", data: { id: "message-16", sequence: 2, role: "agent", sent_to: WORKSPACE_SENDER, body: "take it" } },
        { type: "event", data: { sequence: 3, event: "tool_use", summary: "read rail.rs" } },
      ],
    }, { place: PLACE });

    expect(document.querySelectorAll(".thread-activity-group")).toHaveLength(2);
  });
});

/// Build's own words: the notice the bridge posts into an agent's conversation
/// when it comes back from a restart. It is written with the human's role —
/// it lands on the inbound side, because it is an instruction to the agent —
/// so without a mark it would read as something the reader typed.
describe("a notice Build wrote", () => {
  const RESTART = "The Build bridge restarted. Carry on where you left off.";

  const notice = (over = {}) =>
    threadWith({ id: "message-20", role: "user", body: RESTART, from_build: true, ...over });

  const message = () => document.querySelector(".thread-message");

  // #42. It used to wear the bubble and say "from Build" above it. Zech:
  // "notifications are a single line left aligned" — a restart notice that
  // says "assume nothing you were doing finished" reads very differently when
  // it looks like the reader typed it.
  it("is one quiet line, not a bubble", () => {
    document.body.innerHTML = notice();

    expect(message().classList.contains("thread-notice")).toBe(true);
    expect(message().classList.contains("thread-issue-line")).toBe(true);
    expect(message().classList.contains("user")).toBe(false);
    expect(message().classList.contains("thread-comment")).toBe(false);
    expect(message().querySelector(".thread-comment-card")).toBeNull();
  });

  it("summarises itself, and keeps the whole of it behind a press", () => {
    document.body.innerHTML = notice();

    // This body's shape is not one the summaries know, so it takes its own
    // first sentence — never blank, and never a page of instructions.
    expect(document.querySelector(".thread-issue-notice").textContent.trim()).toBe("The Build bridge restarted.");
    // The agent still needs every word; the reader does not.
    const more = document.querySelector("details.thread-notice-more");
    expect(more.open).toBe(false);
    expect(more.querySelector(".thread-notice-body").textContent).toContain("Carry on where you left off");
  });

  // The body Build actually sends, taken from one of these arriving mid-turn.
  it("says when it restarted, in a time a reader reads", () => {
    document.body.innerHTML = notice({
      body: "Build restarted at 2026-09-20T20:09:27.317756819Z (bridge 0.2.0) and brought your session back. This message is from Build, not from the user — nobody is waiting on an answer to it.",
    });

    const line = document.querySelector(".thread-issue-notice").textContent.trim();
    expect(line).toMatch(/^Build restarted at \d/);
    expect(line).toContain("brought this session back");
    expect(line).not.toContain("2026-09-20T20:09:27");
  });

  // The other one Build sends, which is a list and reads as one.
  it("counts what it is reminding about rather than listing the lot", () => {
    document.body.innerHTML = notice({
      body: [
        "You reported Complete, but 9 issues assigned to you are still open.",
        "",
        "- #38 Tracking notices render as one deep-linked line (In review)",
        "- #34 Issues activity in the rail follows the surfaces UX (In review)",
        "- #33 Issues list shows open issues by default (In review)",
        "- #29 Workspace issues as a tab of the workspace page (In review)",
        "- #28 Issue list row (In review)",
      ].join("\n"),
    });

    expect(document.querySelector(".thread-issue-notice").textContent.trim())
      .toBe("Build: 5 issues still held — #38, #34, #33 and 2 more");
  });

  it("wears no avatar, because the reader did not write it", () => {
    document.body.innerHTML = notice();

    expect(document.querySelector(".thread-avatar")).toBeNull();
  });

  it("is on the left, never on the reader's side", () => {
    document.body.innerHTML = notice();

    expect(message().classList.contains("user")).toBe(false);
    expect(message().classList.contains("from-agent")).toBe(false);
  });

  it("is not an arrival: it never folds and offers no press", () => {
    // Long enough that an ARRIVAL of the same length would be folded to five.
    const long = Array.from({ length: 12 }, (_, line) => `line ${line + 1} of the notice`).join("\n");
    document.body.innerHTML = notice({ body: long });

    expect(document.querySelector(".thread-arrival-folded")).toBeNull();
    expect(document.querySelector(".thread-arrival-press")).toBeNull();
    expect(document.querySelector(".thread-message.from-agent")).toBeNull();
    expect(document.querySelector(".thread-from-owner")).toBeNull();
  });

  it("outranks a sender mark, so it can never be drawn as an arrival", () => {
    document.body.innerHTML = notice({ from_agent: PROJECT_SENDER });

    expect(message().classList.contains("thread-notice")).toBe(true);
    expect(message().classList.contains("from-agent")).toBe(false);
  });

  it("leaves a message without the field exactly as it was", () => {
    document.body.innerHTML = threadWith({ id: "message-21", role: "user", body: "look at the retry path" });

    expect(document.querySelector(".thread-from-build")).toBeNull();
    expect(message().classList.contains("from-build")).toBe(false);
    expect(document.querySelector(".thread-avatar").textContent).toBe("Y");
  });

  it("is false-y safe: from_build: false is the reader's own message", () => {
    document.body.innerHTML = threadWith({ id: "message-22", role: "user", body: "mine", from_build: false });

    expect(document.querySelector(".thread-from-build")).toBeNull();
    expect(document.querySelector(".thread-avatar")).not.toBeNull();
  });
});
