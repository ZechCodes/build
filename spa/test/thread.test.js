// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { acknowledgeProvisionalItem, createThreadCache, createThreadState, currentRevisionId, formatRelativeDate, mergeThreadItems, provisionalThreadItem, threadHtml, threadItemKey, windowFromThreadPayload, wireThreadAttachments, wireThreadComposer, wireThreadLinks, wireThreadRevisionLinks, withoutProvisionalItem } from "../src/core/thread.js";
import { composerHtml } from "../src/core/composer.js";
import { diffThreadMessages } from "../src/core/notes.js";

describe("conversation thread rendering", () => {
  it("puts only delivery status and time in the message footer", () => {
    document.body.innerHTML = threadHtml({ items: [
      { type: "message", data: { role: "user", body: "sent", created_at: "2026-07-24T12:00:00Z" } },
      { type: "message", data: { role: "user", body: "read", created_at: "2026-07-24T12:01:00Z", seen_at: "now" } },
      { type: "message", data: { role: "agent", body: "reply", created_at: "2026-07-24T12:02:00Z" } },
    ] });
    const messages = [...document.querySelectorAll(".thread-message")];

    expect(document.querySelector(".thread-message-head")).toBeNull();
    expect(messages[0].querySelector(".thread-status").ariaLabel).toBe("Sent");
    expect(messages[0].querySelectorAll(".thread-status svg")).toHaveLength(1);
    expect(messages[1].querySelector(".thread-status").ariaLabel).toBe("Read");
    expect(messages[1].querySelectorAll(".thread-status svg")).toHaveLength(2);
    expect(messages[2].querySelector(".thread-status")).toBeNull();
    expect([...messages[0].querySelector(".thread-message-footer").children].map((node) => node.tagName))
      .toEqual(["SPAN", "TIME"]);
  });

  it("renders each persisted native delivery state on the user's message", () => {
    const statuses = ["queued", "submitted", "sent", "seen", "uncertain", "failed"];
    document.body.innerHTML = threadHtml({
      items: statuses.map((delivery_status, index) => ({
        type: "message",
        data: { sequence: index + 1, role: "user", body: delivery_status, delivery_status },
      })),
    });

    const markers = [...document.querySelectorAll(".delivery-status")];
    expect(markers.map((marker) => marker.dataset.deliveryStatus)).toEqual(statuses);
    expect(markers.map((marker) => marker.textContent)).toEqual([
      "Queued", "Queued", "Sent", "Seen", "Delivery uncertain", "Failed",
    ]);
    expect(markers.map((marker) => marker.getAttribute("aria-label"))).toEqual([
      "Queued for the agent",
      "Queued for the agent",
      "Sent to the agent",
      "Seen by the agent",
      "Message delivery is uncertain",
      "Message delivery failed",
    ]);
  });

  it("keeps the historical seen marker when native delivery status is absent", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "message", data: { role: "user", body: "legacy", seen_at: "now" } }],
    });

    expect(document.querySelector(".thread-status").getAttribute("aria-label")).toBe("Read");
    expect(document.querySelector(".delivery-status")).toBeNull();

  });

  it("renders messages, zero-token events, seen state, and revision resolution", async () => {
    const thread = {
      revisions: [{ id: "diff-revision-2-abcd", artifact: "diff" }],
      items: [
        { type: "message", data: { role: "user", body: "rename this", created_at: "2026-07-24T12:00:00Z", seen_at: "now", resolved_by_revision: "diff-revision-2-abcd", anchor: { path: "src/a.js", line_start: 4, line_end: 4 } } },
        { type: "message", data: { role: "agent", body: "Which name?", created_at: "2026-07-24T12:01:00Z" } },
        { type: "event", data: { event: "revision_created", created_at: "2026-07-24T12:02:00Z", revision_id: "diff-revision-2-abcd" } },
      ],
    };
    const html = threadHtml(thread);
    expect(html).toContain("rename this");
    expect(html).toContain('aria-label="Read"');
    expect(html).toContain("Resolved in diff-revision-2-abcd");
    expect(html).toContain("Which name?");
    expect(html).toContain("Revision created");
    expect(currentRevisionId(thread, "diff")).toBe("diff-revision-2-abcd");

    document.body.innerHTML = html;
    expect(document.querySelector(".thread-items").classList.contains("thread-timeline")).toBe(true);
    expect(document.querySelectorAll(".thread-comment")).toHaveLength(2);
    expect(document.querySelectorAll(".thread-avatar")).toHaveLength(2);
    expect(document.querySelector("time").dateTime).toBe("2026-07-24T12:00:00Z");
    wireThreadRevisionLinks(document.body, async (revisionId) => ({ revision_id: revisionId, contents: "+renamed" }));
    document.querySelector(".thread-revision-link").click();
    await Promise.resolve();
    expect(document.querySelector(".thread-revision-view").textContent).toContain("+renamed");
  });

  it("renders blockers and programmatic activity as issue timeline actions", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "blocked", summary: "Needs production credentials", created_at: "2026-07-24T12:00:00Z" } },
        { type: "event", data: { event: "review_blocked", summary: "The migration is not reversible", created_at: "2026-07-24T12:01:00Z" } },
        { type: "event", data: { event: "approved", summary: "Plan approved", created_at: "2026-07-24T12:02:00Z" } },
      ],
    });

    const actions = [...document.querySelectorAll(".thread-event")];
    expect(actions).toHaveLength(3);
    expect(actions[0].classList.contains("blocked")).toBe(true);
    expect(actions[0].textContent).toContain("Agent reported a blocker");
    expect(actions[1].classList.contains("blocked")).toBe(true);
    expect(actions[1].textContent).toContain("Review blocked");
    expect(actions[2].textContent).toContain("Plan approved");
    expect(document.querySelectorAll(".thread-event-icon")).toHaveLength(3);
  });

  const REFERENCE_ITEMS = [
    { type: "message", data: { role: "agent", body: "Changed the parser.", links: [{ kind: "file", path: "src/parser.js", line_start: 8, line_end: 12 }] } },
    { type: "event", data: { event: "stage_started", summary: "Started parser stage", links: [{ kind: "plan_stage", plan_id: "plan-1", stage_id: "parser", path: ".build/plan/01-parser.md" }] } },
  ];
  const FILE_LINK = { kind: "file", path: "src/parser.js", line_start: 8, line_end: 12 };
  const STAGE_LINK = { kind: "plan_stage", plan_id: "plan-1", stage_id: "parser", path: ".build/plan/01-parser.md" };
  const fileRoute = { name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", sourceId: "repo", tab: "files", file: "src/parser.js", line: 8 };

  it("renders a file reference as a link and every other kind as a button", () => {
    document.body.innerHTML = threadHtml({ items: REFERENCE_ITEMS });
    const links = [...document.querySelectorAll(".thread-reference")];
    expect(links).toHaveLength(2);
    expect(links[0].tagName).toBe("A");
    expect(links[0].textContent).toContain("src/parser.js:8-12");
    expect(links[1].tagName).toBe("BUTTON");
    expect(links[1].textContent).toContain(".build/plan/01-parser.md");
  });

  // A file chip is a real link, so the browser's own gestures on it work:
  // middle-click and Cmd-click open the file in another tab. The href is the
  // route the conversation's context names (core/threadLinks.js), written as a
  // URL the router reads back.
  it("gives a file reference the href its route names, and none to the rest", () => {
    document.body.innerHTML = threadHtml({ items: REFERENCE_ITEMS });
    const asked = [];
    wireThreadLinks(document.body, () => {}, (link) => {
      asked.push(link);
      return link.kind === "file" ? fileRoute : null;
    });
    const links = [...document.querySelectorAll(".thread-reference")];
    expect(links[0].getAttribute("href")).toBe(
      "#/device/dev-1/project/p1/workspace/ws-1/directory/repo/files?path=src%2Fparser.js&line=8",
    );
    expect(links[1].hasAttribute("href")).toBe(false);
    // Only the anchors are asked: a button has nowhere to put an answer.
    expect(asked).toEqual([FILE_LINK]);
  });

  it("navigates in the app on a plain press and reads the reference back off the row", () => {
    document.body.innerHTML = threadHtml({ items: REFERENCE_ITEMS });
    const opened = [];
    wireThreadLinks(document.body, (link) => opened.push(link), () => fileRoute);
    const links = [...document.querySelectorAll(".thread-reference")];
    const press = new MouseEvent("click", { bubbles: true, cancelable: true });
    links[0].dispatchEvent(press);
    links[1].click();
    expect(press.defaultPrevented).toBe(true);
    expect(opened).toEqual([FILE_LINK, STAGE_LINK]);
  });

  // A modified press means "somewhere else" — another tab, another window. The
  // app must not swallow it, or the chip is a link that cannot be opened as one.
  it("leaves a modified or middle press to the browser", () => {
    document.body.innerHTML = threadHtml({ items: REFERENCE_ITEMS });
    const opened = [];
    wireThreadLinks(document.body, (link) => opened.push(link), () => fileRoute);
    const chip = document.querySelector("a.thread-reference");
    for (const press of [
      new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true }),
      new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true }),
      new MouseEvent("click", { bubbles: true, cancelable: true, shiftKey: true }),
      new MouseEvent("click", { bubbles: true, cancelable: true, button: 1 }),
    ]) {
      chip.dispatchEvent(press);
      expect(press.defaultPrevented).toBe(false);
    }
    expect(opened).toEqual([]);
  });

  // A conversation about no checkout (an issue) names no route for a file, and
  // a caller that asks for none is the surfaces that only ever opened work
  // items. Either way the press still goes to the app.
  it("still opens a reference the context cannot write a URL for", () => {
    document.body.innerHTML = threadHtml({ items: REFERENCE_ITEMS });
    const opened = [];
    wireThreadLinks(document.body, (link) => opened.push(link));
    const chip = document.querySelector("a.thread-reference");
    expect(chip.hasAttribute("href")).toBe(false);
    chip.click();
    expect(opened).toEqual([FILE_LINK]);
  });

  // The bridge sends a `triaged` event when a review-prioritization pass
  // finishes. It is status, not a hand-back, so it reads as a fact about the
  // diff and carries none of the tone a blocked or done entry does.
  it("names a triage pass rather than falling back to its wire token", () => {
    document.body.innerHTML = threadHtml({
      items: [
        {
          type: "event",
          data: { event: "triaged", summary: "the crypto change carries the risk" },
        },
      ],
    });
    const entry = document.querySelector(".thread-event");
    expect(entry.textContent).toContain("Diff ordered for review");
    expect(entry.textContent).toContain("the crypto change carries the risk");
    expect(entry.classList.contains("blocked")).toBe(false);
    expect(entry.classList.contains("success")).toBe(false);
  });

  // And a `triage_overridden` event when the reviewer disagrees with where the
  // pass put a hunk. It is the same shape of fact: the agent is told, and
  // nothing is asked of anyone.
  it("names a reviewer's disagreement with a triage pass", () => {
    document.body.innerHTML = threadHtml({
      items: [
        {
          type: "event",
          data: {
            event: "triage_overridden",
            summary: "The reviewer opened src/crypto.rs: triage collapsed a change that needed reading.",
          },
        },
      ],
    });
    const entry = document.querySelector(".thread-event");
    expect(entry.textContent).toContain("Review order corrected");
    expect(entry.textContent).toContain("src/crypto.rs");
    expect(entry.classList.contains("blocked")).toBe(false);
    expect(entry.classList.contains("success")).toBe(false);
  });

  // The whole disagreement, in the conversation: which file, the claim the pass
  // made about it, and what the reviewer said back — each its own line, so the
  // agent reading this can see which of its own claims was not believed.
  it("shows the hunk's context on an override: the file, the rejected rationale, the reviewer's note", () => {
    document.body.innerHTML = threadHtml({
      items: [
        {
          type: "event",
          data: {
            event: "triage_overridden",
            summary:
              "The reviewer opened src/crypto.rs: triage collapsed a change that needed reading.\n\n" +
              "Triage said: a mechanical rename\n\nkey derivation is never boilerplate",
          },
        },
      ],
    });
    const detail = document.querySelector(".thread-event .thread-event-detail");
    expect(detail.textContent).toContain("src/crypto.rs");
    expect(detail.textContent).toContain("Triage said: a mechanical rename");
    expect(detail.textContent).toContain("key derivation is never boilerplate");
    // Three claims, three paragraphs — not one run-on line.
    expect(detail.querySelectorAll("p").length).toBe(3);
  });

  it("renders a done-flagged send like any other message after the done entry", () => {
    const html = threadHtml(
      {
        items: [
          { type: "event", data: { event: "done", summary: "Implemented persistent review conversations." } },
          { type: "message", data: { role: "agent", done: true, body: "Implemented persistent review conversations." } },
        ],
        last_completion: {
          critical_files: ["src/plan.js"],
          risk_notes: ["Keep the durable thread intact."],
          decisions: [],
          skips: [],
        },
      },
      { initialMessage: "Make review conversations persistent", composer: true },
    );

    document.body.innerHTML = html;
    const timeline = [...document.querySelector(".thread-items").children];
    const messages = [...document.querySelectorAll(".thread-message")];
    expect(messages[0].classList.contains("user")).toBe(true);
    expect(messages[0].textContent).toContain("Make review conversations persistent");
    expect(timeline.at(-2).classList.contains("thread-event")).toBe(true);
    expect(timeline.at(-2).textContent).toContain("Agent reported done");
    expect(timeline.at(-1).classList.contains("thread-message")).toBe(true);
    expect(timeline.at(-1).classList.contains("thread-completion")).toBe(false);
    expect(timeline.at(-1).querySelector(".thread-message-head")).toBeNull();
    expect(timeline.at(-1).textContent).toContain("Implemented persistent review conversations.");
    expect(timeline.at(-1).textContent).not.toContain("Critical files");
    expect(timeline.at(-1).textContent).not.toContain("src/plan.js");
    expect(document.querySelector(".thread-event-detail")).toBeNull();
    expect(document.querySelector("details")).toBeNull();
    expect(document.querySelector("#planthreadinput")).not.toBeNull();
    expect(document.querySelector("#planthreadsend").getAttribute("aria-label")).toBe("Send message");
  });

  // The timeline's avatar spine is drawn by the timeline itself, so a
  // conversation with nothing in it drew a 2px rule down the side of its own
  // empty state — a thread stem holding no messages. The empty case marks
  // itself so the CSS can drop the spine and the gutter it aligns to.
  it("marks an empty conversation, so its avatar spine is not drawn against nothing", () => {
    document.body.innerHTML = threadHtml({ items: [] }, { composer: true });
    expect(document.querySelector(".thread-timeline").classList.contains("is-empty")).toBe(true);
    expect(document.querySelector(".review-thread").classList.contains("is-empty")).toBe(true);
    expect(document.querySelector(".thread-empty").textContent).toBe("No conversation yet.");
  });

  it("drops the empty mark as soon as there is anything on the record", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "message", data: { role: "user", body: "rename this", created_at: "2026-07-24T12:00:00Z" } }],
    });
    expect(document.querySelector(".thread-timeline").classList.contains("is-empty")).toBe(false);
    expect(document.querySelector(".review-thread").classList.contains("is-empty")).toBe(false);
    expect(document.querySelector(".thread-empty")).toBeNull();
  });

  // An initial message is a rendered item like any other: a plan opened from an
  // issue has a conversation from its first frame.
  it("is not empty when the only item is the initial message folded in", () => {
    document.body.innerHTML = threadHtml({ items: [] }, { initialMessage: "add a dark theme" });
    expect(document.querySelector(".thread-timeline").classList.contains("is-empty")).toBe(false);
  });

  it("renders a caller-scoped composer so two composers can coexist without id collisions", () => {
    document.body.innerHTML =
      threadHtml({ items: [] }, { composer: true }) +
      threadHtml(
        { items: [] },
        {
          composer: {
            inputId: "diffthreadinput",
            sendId: "diffthreadsend",
            hintId: "diffthreadhint",
            placeholder: "Ask the coding agent…",
          },
        },
      );
    expect(document.querySelectorAll("#planthreadinput")).toHaveLength(1);
    expect(document.querySelectorAll("#diffthreadinput")).toHaveLength(1);
    expect(document.querySelector("#diffthreadinput").placeholder).toBe("Ask the coding agent…");
    // One row to start: the box grows to what is typed into it rather than
    // sitting open at a height nothing has filled yet.
    expect(document.querySelector("#diffthreadinput").rows).toBe(1);
    expect(document.querySelector("#diffthreadhint")).not.toBeNull();
    expect(document.querySelector("#diffthreadsend").getAttribute("aria-label")).toBe("Send message");
    expect(document.querySelector("#diffthreadsend").classList.contains("composer-send")).toBe(true);
  });

  it("does not duplicate a sequenced completion message with the legacy fallback", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "done", summary: "Implemented the requested change." } },
        { type: "message", data: { role: "agent", source: "completion", body: "Completion report\n\nCritical files\n- src/app.rs" } },
      ],
      last_completion: { critical_files: ["src/app.rs"], risk_notes: [], decisions: [], skips: [] },
    }, { initialMessage: "Fix the bug" });
    expect(document.querySelectorAll(".thread-completion")).toHaveLength(0);
  });

  it("identifies the harness that reported completion", () => {
    document.body.innerHTML = threadHtml({
      sessions: [{ provider: "Codex CLI" }],
      items: [
        { type: "event", data: { event: "session_ended" } },
        { type: "event", data: { event: "done", summary: "Finished the task." } },
        { type: "message", data: { role: "agent", done: true, body: "Finished the task." } },
      ],
    }, { initialMessage: "Do the task" });
    expect(document.querySelector(".thread-message-head")).toBeNull();
    expect(document.querySelector(".thread-items").textContent).toContain("Codex TUI session ended");
    expect(document.querySelector(".thread-items").textContent).toContain("Codex TUI reported done");
  });

  it("calls either claude carrier Claude Code, never the word the wire uses", () => {
    // Claude is Claude: which program carried the session is the bridge's
    // record, not a second agent for a reader to tell apart.
    document.body.innerHTML = threadHtml({
      sessions: [{ provider: "claude_adk" }],
      items: [
        { type: "event", data: { event: "session_ended" } },
        { type: "message", data: { role: "agent", done: true, body: "Finished the task." } },
      ],
    }, { initialMessage: "Do the task" });
    const shown = document.querySelector(".thread-items").textContent;
    expect(shown).toContain("Claude Code session ended");
    expect(shown).not.toMatch(/claude_adk/);
    expect(shown).not.toMatch(/headless/i);
  });
});

describe("the startup events the status line has taken over", () => {
  it("keeps them out of the timeline and paints every other kind", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "session_started", created_at: "2026-07-24T12:00:00Z" } },
        { type: "event", data: { event: "run_started", created_at: "2026-07-24T12:01:00Z" } },
        { type: "event", data: { event: "session_ended", created_at: "2026-07-24T12:02:00Z" } },
        { type: "message", data: { role: "agent", body: "on it", created_at: "2026-07-24T12:03:00Z" } },
      ],
    });
    const shown = document.querySelector(".thread-items").textContent;
    expect(shown).not.toContain("session started");
    expect(shown).not.toContain("Run started");
    expect(shown).toContain("Agent session ended");
    expect(shown).toContain("on it");
    expect(document.querySelectorAll(".thread-event")).toHaveLength(1);
  });

  it("leaves a conversation of nothing but startup events empty", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "run_started", created_at: "2026-07-24T12:00:00Z" } }],
    });
    expect(document.querySelector(".thread-empty")).toBeTruthy();
  });
});

describe("relative conversation dates", () => {
  const localDate = (year, month, day, hour = 12, minute = 0) =>
    new Date(year, month - 1, day, hour, minute);
  const now = localDate(2026, 7, 29, 21);

  it.each([
    [new Date(now.getTime() - 30_000), "Just now"],
    [new Date(now.getTime() - 5 * 60_000), "5 minutes ago"],
    [new Date(now.getTime() - 4 * 60 * 60_000), "4 hours ago"],
    [localDate(2026, 7, 28, 20), "Yesterday at 8pm"],
    [localDate(2026, 7, 27), "Monday"],
    [localDate(2026, 5, 5), "May 5th"],
    [localDate(2025, 6, 7), "June 7th, 2025"],
  ])("formats %s as %s", (date, expected) => {
    expect(formatRelativeDate(date, now)).toBe(expected);
  });

  it("uses the relative date in rendered thread timestamps", () => {
    const createdAt = new Date(Date.now() - 5 * 60_000).toISOString();
    document.body.innerHTML = threadHtml({
      items: [{ type: "event", data: { event: "approved", created_at: createdAt } }],
    });

    expect(document.querySelector("time").textContent).toBe("5 minutes ago");
  });
});

// A message this tab has sent stands in the conversation before the wire has
// echoed it back. It lives in the same record as every other item — views hold
// no second store — under a key made of the operation that is carrying it, and
// it leaves the record the moment the real item arrives.
describe("provisional items in the thread record", () => {
  const item = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
  const sent = (operationId, body, sequence = null) => provisionalThreadItem({
    operationId,
    message: { body },
    sequence,
  });

  it("keys a provisional item by its operation and a real one by its sequence", () => {
    expect(threadItemKey(item(4, "a"))).toBe("4");
    expect(threadItemKey(sent("op-1", "ship it"))).toBe("provisional:op-1");
    // The key is the operation's whatever the post has said so far: an
    // acknowledged send is still this tab's stand-in until the item itself
    // arrives, and rekeying it to the sequence would leave both on screen.
    expect(threadItemKey(sent("op-1", "ship it", 9))).toBe("provisional:op-1");
  });

  it("carries the words, the attachments and the queued mark the panel draws", () => {
    const message = sent("op-1", "ship it");
    expect(message.type).toBe("message");
    expect(message.data.role).toBe("user");
    expect(message.data.body).toBe("ship it");
    expect(message.data.sequence).toBeNull();
    expect(message.data.delivery_status).toBe("queued");
    expect(message.data.operation_id).toBe("op-1");
  });

  it("stands a provisional item after everything the wire has sent", () => {
    const merged = mergeThreadItems([item(1, "a"), item(3, "c")], [sent("op-1", "ship it")]);
    expect(merged.map(threadItemKey)).toEqual(["1", "3", "provisional:op-1"]);
  });

  it("replaces a provisional item with the real item carrying its operation", () => {
    const held = [item(1, "a"), sent("op-1", "ship it")];
    const arrived = { type: "message", data: { sequence: 4, role: "user", body: "ship it", operation_id: "op-1" } };

    const merged = mergeThreadItems(held, [arrived]);

    expect(merged.map(threadItemKey)).toEqual(["1", "4"]);
    expect(merged[1].data.body).toBe("ship it");
  });

  // Nothing on the wire carries an operation id on the item itself. What the
  // post answers with is the sequence it was written at, so a provisional item
  // that has been acknowledged knows which arrival is its own — and the
  // arrival takes its place in sequence order rather than at the end.
  it("replaces an acknowledged provisional item with the item at its sequence", () => {
    const held = [item(1, "a"), item(5, "e"), sent("op-1", "ship it", 6)];
    const merged = mergeThreadItems(held, [item(6, "ship it"), item(7, "reply")]);

    expect(merged.map(threadItemKey)).toEqual(["1", "5", "6", "7"]);
    expect(merged.map((entry) => entry.data.body)).toEqual(["a", "e", "ship it", "reply"]);
  });

  it("keeps the record in sequence order whatever order an arrival is in", () => {
    const merged = mergeThreadItems([item(2, "b")], [item(4, "d"), item(1, "a")]);
    expect(merged.map(threadItemKey)).toEqual(["1", "2", "4"]);
  });

  it("lets an arrived copy of a held item win, without growing the record", () => {
    const merged = mergeThreadItems([item(1, "a"), item(2, "stale")], [item(2, "reshipped")]);
    expect(merged.map(threadItemKey)).toEqual(["1", "2"]);
    expect(merged[1].data.body).toBe("reshipped");
  });

  it("drops a provisional item by its operation, for a send that was refused", () => {
    const held = [item(1, "a"), sent("op-1", "ship it")];
    expect(withoutProvisionalItem(held, "op-1").map(threadItemKey)).toEqual(["1"]);
    expect(withoutProvisionalItem(held, "op-2")).toBe(held);
  });

  it("stamps the sequence a post was acknowledged at onto the item waiting for it", () => {
    const held = [item(1, "a"), sent("op-1", "ship it")];
    const acknowledged = acknowledgeProvisionalItem(held, "op-1", 6);
    expect(acknowledged.map(threadItemKey)).toEqual(["1", "provisional:op-1"]);
    expect(acknowledged[1].data.sequence).toBe(6);
    expect(acknowledged[1].data.delivery_status).toBe("sent");
  });
});

describe("structured review messages", () => {
  it("preserves diff anchors instead of flattening them into a prompt", () => {
    expect(diffThreadMessages([{ file: "src/a.js", lnA: 2, lnB: 4, snippet: "old()", comment: "rename" }], "ship safely", "diff-r1"))
      .toEqual([
        { body: "rename", anchor: { artifact: "diff", revision_id: "diff-r1", path: "src/a.js", side: "new", line_start: 2, line_end: 4, heading_path: [], snippet: "old()" } },
        { body: "ship safely", anchor: null },
      ]);
  });
});

describe("attachments on the record", () => {
  const withAttachments = (attachments) => ({
    items: [{ type: "message", data: { role: "user", body: "look at this", created_at: "2026-08-09T12:00:00Z", attachments } }],
  });

  it("shows an image inline and everything else as a chip you can open", () => {
    const html = threadHtml(withAttachments([
      { name: "screenshot.png", path: ".build/attachments/ab12-screenshot.png", mime: "image/png", size: 40960 },
      { name: "trace.txt", path: ".build/attachments/cd34-trace.txt", mime: "text/plain", size: 2048 },
    ]));
    expect(html).toContain('data-attachment-path=".build/attachments/ab12-screenshot.png"');
    expect(html).toContain("thread-attachment-image");
    expect(html).toContain("trace.txt");
    expect(html).toContain("2 KB");
  });

  it("gives a sent file the tile for its kind, and a picture its size", () => {
    const html = threadHtml(withAttachments([
      { name: "screenshot.png", path: ".build/attachments/ab12-screenshot.png", mime: "image/png", size: 40960 },
      { name: "server.py", path: ".build/attachments/cd34-server.py", mime: "application/octet-stream", size: 2048 },
      { name: "logs.tar.gz", path: ".build/attachments/ef56-logs.tar.gz", mime: "application/gzip", size: 5000000 },
    ]));
    document.body.innerHTML = html;
    const chips = [...document.querySelectorAll(".thread-attachment")];
    expect(chips.map((chip) => chip.querySelector(".attachment-glyph").dataset.kind)).toEqual(["code", "archive"]);
    expect(chips[0].querySelector(".attachment-glyph-tag").textContent).toBe("PY");
    expect(chips[0].querySelector(".attachment-glyph svg")).toBeTruthy();
    expect(chips[1].querySelector(".thread-attachment-size").textContent).toBe("5.0 MB");
    expect(document.querySelector(".thread-attachment-figure figcaption").textContent).toContain("41 KB");
  });

  it("opens an image in a dismissible lightbox and returns focus", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    document.body.innerHTML = threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]), { threadState });
    const preview = document.querySelector(".thread-attachment-preview");
    wireThreadAttachments(document.body, async () => ({ mime: "image/png", content_b64: "AAAA" }), threadState);
    preview.focus();
    preview.click();
    await Promise.resolve();
    await Promise.resolve();

    const lightbox = document.querySelector(".thread-lightbox");
    expect(lightbox.querySelector("img").src).toBe("data:image/png;base64,AAAA");
    expect(lightbox.querySelector("img").alt).toBe("shot.png");
    lightbox.querySelector("button").focus();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.querySelector(".thread-lightbox")).toBeNull();
    expect(document.activeElement).toBe(preview);
  });

  it("escapes an attachment name rather than rendering it", () => {
    const html = threadHtml(withAttachments([
      { name: '<img src=x onerror="boom">.png', path: ".build/attachments/x.png", mime: "image/png", size: 1 },
    ]));
    expect(html).not.toContain("onerror=\"boom\"");
    expect(html).toContain("&lt;img");
  });

  it("fills an inline image from the bytes the bridge hands back, once", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    document.body.innerHTML = `<div id="host">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]))}</div>`;
    const host = document.querySelector("#host");
    const asked = [];
    const load = (path) => {
      asked.push(path);
      return Promise.resolve({ mime: "image/png", content_b64: "AAAA" });
    };

    wireThreadAttachments(host, load, threadState);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(host.querySelector("img.thread-attachment-image").getAttribute("src")).toBe("data:image/png;base64,AAAA");

    // A polling surface re-renders the timeline constantly; the bytes are
    // content-addressed and immutable, so asking twice is pure waste.
    document.body.innerHTML = `<div id="host">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]))}</div>`;
    wireThreadAttachments(document.querySelector("#host"), load, threadState);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(asked).toEqual([".build/attachments/ab12-shot.png"]);
  });

  it("shares an in-flight attachment load within its conversation", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    document.body.innerHTML = `<div id="first">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]), { threadState })}</div><div id="second">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]), { threadState })}</div>`;
    let release;
    const load = vi.fn(() => new Promise((resolve) => (release = resolve)));

    wireThreadAttachments(document.querySelector("#first"), load, threadState);
    wireThreadAttachments(document.querySelector("#second"), load, threadState);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(load).toHaveBeenCalledTimes(1);

    release({ mime: "image/png", content_b64: "AAAA" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.querySelector("#first img").getAttribute("src")).toBe("data:image/png;base64,AAAA");
    expect(document.querySelector("#second img").getAttribute("src")).toBe("data:image/png;base64,AAAA");
  });
});

describe("thread composer wiring", () => {
  const mount = (ids = { input: "planthreadinput", send: "planthreadsend", hint: "planthreadhint" }) => {
    document.body.innerHTML = `<div id="host">
      <textarea id="${ids.input}"></textarea>
      <span id="${ids.hint}"></span>
      <button id="${ids.send}">Send</button>
    </div>`;
    return document.querySelector("#host");
  };
  const cmdEnter = (input) => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true }));

  const mountActionControl = () => {
    document.body.innerHTML = `<div id="host">${composerHtml({
      inputId: "ci", sendId: "cs", hintId: "ch", placeholder: "Say something…", canInterrupt: true,
    })}</div>`;
    return document.querySelector("#host");
  };

  it("changes Stop to Send while typing and follows a turn that ends during the stop request", async () => {
    const host = mountActionControl();
    let finishInterrupt;
    const control = wireThreadComposer(host, {
      ids: { input: "ci", send: "cs", hint: "ch" },
      readDraft: () => "",
      writeDraft: () => {},
      onSubmit: () => Promise.resolve(),
      onInterrupt: () => new Promise((resolve) => { finishInterrupt = resolve; }),
    });
    expect(host.querySelector("#cs").dataset.action).toBe("stop");

    const input = host.querySelector("#ci");
    input.value = "new direction";
    input.dispatchEvent(new Event("input"));
    expect(host.querySelector("#cs").dataset.action).toBe("send");

    input.value = "";
    input.dispatchEvent(new Event("input"));
    host.querySelector("#cs").click();
    control.setCanInterrupt(false);
    expect(host.querySelector("#cs").disabled).toBe(true);
    finishInterrupt();
    await Promise.resolve();
    await Promise.resolve();
    expect(host.querySelector("#cs").dataset.action).toBe("send");
    expect(host.querySelector("#cs").disabled).toBe(false);
  });

  it("posts once when Cmd+Enter is pressed repeatedly during an in-flight send", async () => {
    // Cmd+Enter bypasses the button's native disabled gate, so without an
    // explicit re-entry guard the obvious retry double-posts.
    const host = mount();
    let resolveSend;
    const sent = [];
    wireThreadComposer(host, {
      ids: { input: "planthreadinput", send: "planthreadsend", hint: "planthreadhint" },
      readDraft: () => "",
      writeDraft: () => {},
      onSubmit: (body) => {
        sent.push(body);
        return new Promise((resolve) => (resolveSend = resolve));
      },
    });
    const input = host.querySelector("#planthreadinput");
    input.value = "ship it";
    cmdEnter(input);
    cmdEnter(input);
    cmdEnter(input);
    expect(sent).toEqual(["ship it"]);

    resolveSend();
    await Promise.resolve();
    await Promise.resolve();
    expect(input.value).toBe("");
    expect(host.querySelector("#planthreadsend").disabled).toBe(false);
    expect(host.querySelector("#planthreadsend").textContent).toBe("Send");
  });

  it("restores the composer and keeps the text when the send fails", async () => {
    const host = mount();
    wireThreadComposer(host, {
      ids: { input: "planthreadinput", send: "planthreadsend", hint: "planthreadhint" },
      readDraft: () => "",
      writeDraft: () => {},
      onSubmit: () => Promise.reject(new Error("relay down")),
      onError: () => {},
    });
    const input = host.querySelector("#planthreadinput");
    input.value = "keep me";
    cmdEnter(input);
    await Promise.resolve();
    await Promise.resolve();
    expect(input.value).toBe("keep me");
    expect(host.querySelector("#planthreadsend").disabled).toBe(false);
  });

  it("refuses an empty body without calling the transport", () => {
    const host = mount();
    let calls = 0;
    wireThreadComposer(host, {
      ids: { input: "planthreadinput", send: "planthreadsend", hint: "planthreadhint" },
      readDraft: () => "",
      writeDraft: () => {},
      onSubmit: () => { calls += 1; return Promise.resolve(); },
    });
    const input = host.querySelector("#planthreadinput");
    input.value = "   ";
    cmdEnter(input);
    expect(calls).toBe(0);
    expect(host.querySelector("#planthreadhint").textContent).toContain("Type a message");
  });
});

describe("sending a message that carries files", () => {
  const mountWithTray = (overrides = {}, composerOverrides = {}) => {
    document.body.innerHTML = `<div id="host">${composerHtml({
      inputId: "ti",
      sendId: "ts",
      hintId: "th",
      placeholder: "Say something…",
      attachable: true,
      ...composerOverrides,
    })}</div>`;
    const host = document.querySelector("#host");
    const sent = [];
    let draft = "";
    let attachments = [];
    wireThreadComposer(host, {
      ids: { input: "ti", send: "ts", hint: "th" },
      readDraft: () => draft,
      writeDraft: (value) => { draft = value; },
      readAttachments: () => attachments,
      writeAttachments: (next) => { attachments = next; },
      upload: (file) => Promise.resolve({ name: file.name, path: `.build/attachments/x-${file.name}`, mime: file.type || "text/plain", size: file.size }),
      onSubmit: (body, files) => { sent.push({ body, files }); return Promise.resolve(); },
      ...overrides,
    });
    return { host, sent };
  };
  const drop = (host, files) => {
    const event = new Event("drop", { bubbles: true, cancelable: true });
    event.dataTransfer = { files, items: [], types: ["Files"] };
    host.dispatchEvent(event);
  };
  const settle = async () => {
    for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  };

  it("changes Stop to Send when a file is attached without any text", async () => {
    const { host } = mountWithTray({}, { canInterrupt: true });
    expect(host.querySelector("#ts").dataset.action).toBe("stop");
    drop(host, [new File(["a"], "shot.png", { type: "image/png" })]);
    await settle();
    expect(host.querySelector("#ts").dataset.action).toBe("send");
  });

  it("names the uploaded files on the send and empties the tray after", async () => {
    const { host, sent } = mountWithTray();
    drop(host, [new File(["a"], "shot.png", { type: "image/png" })]);
    await settle();
    host.querySelector("#ti").value = "see this";
    host.querySelector("#ts").click();
    await settle();

    expect(sent).toEqual([{ body: "see this", files: [{ name: "shot.png", path: ".build/attachments/x-shot.png", mime: "image/png", size: 1 }] }]);
    expect(host.querySelectorAll(".composer-chip")).toHaveLength(0);
  });

  it("sends a file with no words, because the file IS the message", async () => {
    const { host, sent } = mountWithTray();
    drop(host, [new File(["a"], "shot.png", { type: "image/png" })]);
    await settle();
    host.querySelector("#ts").click();
    await settle();

    expect(sent).toHaveLength(1);
    expect(sent[0].body).toBe("");
  });

  it("waits for a file still going up rather than sending a message that points at nothing", async () => {
    const { host, sent } = mountWithTray({ upload: () => new Promise(() => {}) });
    drop(host, [new File(["a"], "slow.png", { type: "image/png" })]);
    await settle();
    host.querySelector("#ti").value = "here";
    host.querySelector("#ts").click();
    await settle();

    expect(sent).toEqual([]);
    expect(host.querySelector("#th").textContent).toContain("still attaching");
  });

  it("keeps the files when the send fails, exactly as it keeps the words", async () => {
    const { host, sent } = mountWithTray({
      onSubmit: () => Promise.reject(new Error("relay down")),
      onError: () => {},
    });
    drop(host, [new File(["a"], "shot.png", { type: "image/png" })]);
    await settle();
    host.querySelector("#ti").value = "see this";
    host.querySelector("#ts").click();
    await settle();

    expect(sent).toEqual([]);
    expect(host.querySelector("#ti").value).toBe("see this");
    expect(host.querySelectorAll(".composer-chip")).toHaveLength(1);
  });
});

// ---- the persisted window ------------------------------------------------------
// The local cache keeps a conversation's window across sessions: an empty
// cache seeds from what was saved, the next detail read is a forward delta
// rather than a first page, and the standing soundness checks self-heal
// anything the time away made stale.
describe("the window a record holds", () => {
  const item = (sequence) => ({ id: `m-${sequence}`, data: { sequence } });
  const saved = {
    items: [item(1), item(2)],
    olderItemsRemain: true,
    deliveredSequence: 2,
    knownTotalItems: 5,
    activityDigests: [],
  };

  it("opens a record and answers what it says about either end", () => {
    const cache = createThreadCache();
    expect(cache.readWindow()).toBeNull();

    expect(cache.seedWindow(saved)).toBe(true);

    expect(cache.readWindow()).toEqual(saved);
    expect(cache.hasOlderItems()).toBe(true);
    expect(cache.windowFloorSequence()).toBe(1);
    expect(cache.olderPageParam()).toEqual({ before_sequence: 1 });
  });

  // The record is the conversation: a write to it is read back whole. A cache
  // that held its own copy would have to be told what changed, which is the
  // cursor merge this stopped being.
  it("replaces what it holds every time the record is opened", () => {
    const cache = createThreadCache();
    cache.seedWindow(saved);

    cache.seedWindow({ items: [item(7)], deliveredSequence: 7, knownTotalItems: 7 });

    expect(cache.readWindow().items.map((held) => held.data.sequence)).toEqual([7]);
    expect(cache.hasOlderItems()).toBe(false);
  });

  it("holds nothing for an empty or malformed record", () => {
    const cache = createThreadCache();
    cache.seedWindow(saved);

    expect(cache.seedWindow(null)).toBe(false);
    expect(cache.readWindow()).toBeNull();
    expect(cache.seedWindow({ items: [] })).toBe(false);
    expect(cache.olderPageParam()).toBeNull();
    expect(cache.windowFloorSequence()).toBeNull();
  });

  it("takes the cursor off the items when the record names none", () => {
    const cache = createThreadCache();
    cache.seedWindow({ items: [item(3), { id: "m-4", data: { sequence: 4, updated_sequence: 6 } }] });
    expect(cache.readWindow().deliveredSequence).toBe(6);
  });

  it("forgets the window when the reader opens another conversation", () => {
    const cache = createThreadCache();
    cache.seedWindow(saved);
    cache.reset();
    expect(cache.readWindow()).toBeNull();
    expect(cache.hasOlderItems()).toBe(false);
  });

  it("shapes a bare thread payload as a saved window", () => {
    const shaped = windowFromThreadPayload({ items: [item(4), item(5)], has_more: true, thread_total: 9 });
    expect(shaped).toEqual({
      items: [item(4), item(5)],
      olderItemsRemain: true,
      deliveredSequence: 5,
      knownTotalItems: 9,
      activityDigests: [],
    });
    expect(windowFromThreadPayload({ items: [] })).toBeNull();
    expect(windowFromThreadPayload(null)).toBeNull();
  });
});

describe("naming a thread item", () => {
  it("names a thread item by its sequence", () => {
    expect(threadItemKey({ type: "message", data: { sequence: 12 } })).toBe("12");
    expect(threadItemKey({ type: "event", data: { sequence: 0 } })).toBe("0");
    expect(threadItemKey({ type: "message", data: {} })).toBe("");
    expect(threadItemKey({})).toBe("");
  });
});

// ---- the digests a window holds --------------------------------------------
// A page ships a bounded slice of every activity run and a digest for the rest
// of it. The window holds the digests the same way it holds the items: a page
// says what a run totals, a forward delta says nothing about one, and an older
// page reaches back to runs the window had never heard of.
describe("the activity digests a window holds", () => {
  const item = (sequence) => ({ id: `m-${sequence}`, data: { sequence } });
  const digest = (from, through, toolCalls) => ({
    from_sequence: from,
    through_sequence: through,
    tool_calls: toolCalls,
    rows: toolCalls,
    last_tool_call: null,
  });

  it("carries a page's digests into the window the record holds", () => {
    const shaped = windowFromThreadPayload({
      items: [item(4), item(5)],
      activity_digests: [digest(1, 5, 40)],
      has_more: true,
      thread_total: 9,
    });

    expect(shaped.activityDigests).toEqual([digest(1, 5, 40)]);
  });

  it("opens them with the window they were saved beside", () => {
    const cache = createThreadCache();
    cache.seedWindow({ items: [item(8), item(9)], deliveredSequence: 9, activityDigests: [digest(4, 9, 1000)] });

    expect(cache.readWindow().activityDigests).toEqual([digest(4, 9, 1000)]);

    cache.reset();
    expect(cache.readWindow()).toBeNull();
  });
});
