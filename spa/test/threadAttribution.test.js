// @vitest-environment jsdom
// Agent-to-agent messages in one agent's conversation.
//
// Three shapes share the timeline and only one of them is the reader's own. The
// human's message keeps the right-hand bubble it always had; another agent's
// words arrive on the left, in green, under the conversation they came from;
// and a message this agent sent out is two lines that open onto what was said.
import { describe, expect, it } from "vitest";
import { createThreadState, threadHtml, wireThreadSentMessages } from "../src/core/thread.js";

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

/// The human's own message, exactly as it was drawn before any of this. The
/// markup is the assertion: the reader's side of the thread is untouched.
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
