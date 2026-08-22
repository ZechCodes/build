// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { FIRST_PAGE_ITEMS, createThreadCache, currentRevisionId, formatRelativeDate, threadHtml, wireThreadAttachments, wireThreadComposer, wireThreadLinks, wireThreadRevisionLinks } from "../src/core/thread.js";
import { composerHtml } from "../src/core/composer.js";
import { diffThreadMessages } from "../src/core/notes.js";

describe("conversation thread rendering", () => {
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
    expect(html).toContain("Seen");
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

  it("renders typed file and stage references and wires them without hrefs", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "message", data: { role: "agent", body: "Changed the parser.", links: [{ kind: "file", path: "src/parser.js", line_start: 8, line_end: 12 }] } },
        { type: "event", data: { event: "stage_started", summary: "Started parser stage", links: [{ kind: "plan_stage", plan_id: "plan-1", stage_id: "parser", path: ".build/plan/01-parser.md" }] } },
      ],
    });
    const links = [...document.querySelectorAll(".thread-reference")];
    expect(links).toHaveLength(2);
    expect(links[0].tagName).toBe("BUTTON");
    expect(links[0].textContent).toContain("src/parser.js:8-12");
    expect(links[1].textContent).toContain(".build/plan/01-parser.md");
    expect(document.querySelector("a")).toBeNull();

    const opened = [];
    wireThreadLinks(document.body, (link) => opened.push(link));
    links[0].click();
    links[1].click();
    expect(opened).toEqual([
      { kind: "file", path: "src/parser.js", line_start: 8, line_end: 12 },
      { kind: "plan_stage", plan_id: "plan-1", stage_id: "parser", path: ".build/plan/01-parser.md" },
    ]);
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
    expect(timeline.at(-1).querySelector(".thread-message-head").textContent).toContain("Agent commented");
    expect(timeline.at(-1).textContent).toContain("Implemented persistent review conversations.");
    expect(timeline.at(-1).textContent).not.toContain("Critical files");
    expect(timeline.at(-1).textContent).not.toContain("src/plan.js");
    expect(document.querySelector(".thread-event-detail")).toBeNull();
    expect(document.querySelector("details")).toBeNull();
    expect(document.querySelector("#planthreadinput")).not.toBeNull();
    expect(document.querySelector("#planthreadsend .composer-send-label").textContent).toBe("Send");
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
    expect(document.querySelector("#diffthreadsend .composer-send-label").textContent).toBe("Send");
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
        { type: "event", data: { event: "session_started" } },
        { type: "event", data: { event: "done", summary: "Finished the task." } },
        { type: "message", data: { role: "agent", done: true, body: "Finished the task." } },
      ],
    }, { initialMessage: "Do the task" });
    const completionHead = [...document.querySelectorAll(".thread-message-head")].at(-1).textContent;
    expect(completionHead).toContain("Codex commented");
    expect(completionHead).not.toContain("Agent commented");
    expect(document.querySelector(".thread-items").textContent).toContain("Codex session started");
    expect(document.querySelector(".thread-items").textContent).toContain("Codex reported done");
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

describe("thread cache (cursor merge for the detail polls)", () => {
  const item = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

  it("starts with a full fetch, then sends the last-known sequence as the cursor", () => {
    const cache = createThreadCache();
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
    const absorbed = cache.absorb({ id: "thread:plan-1", items: [item(1, "hello"), item(3, "world")], revisions: [] });
    expect(absorbed.items.map((i) => i.data.sequence)).toEqual([1, 3]);
    expect(absorbed.revisions).toEqual([]);
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 3 });
  });

  it("appends a cursored delta in sequence order without mutating the payload", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a"), item(2, "b")] });
    const delta = { items: [item(4, "d"), item(3, "c")], thread_total: 4, thread_last_sequence: 4 };
    const merged = cache.absorb(delta);
    expect(merged.items.map((i) => i.data.sequence)).toEqual([1, 2, 3, 4]);
    expect(delta.items.map((i) => i.data.sequence)).toEqual([4, 3]); // payload untouched
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 4 });
  });

  it("never grows on a replayed sequence, and the arrived copy of the item wins", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a"), item(2, "stale")] });
    const merged = cache.absorb({ items: [item(2, "reshipped"), item(3, "c")], thread_total: 3, thread_last_sequence: 3 });
    expect(merged.items.map((i) => i.data.sequence)).toEqual([1, 2, 3]);
    expect(merged.items[1].data.body).toBe("reshipped");
  });

  it("replaces a held item when a mutation delta re-ships it, and advances the cursor past the bump", () => {
    const cache = createThreadCache();
    cache.absorb({
      items: [{ type: "message", data: { sequence: 1, role: "user", body: "rename it", seen_at: null } }],
    });
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 1 });
    // The bridge stamped seen_at on the held message and bumped its
    // updated_sequence; the cursored delta re-ships the newer copy.
    const merged = cache.absorb({
      items: [{ type: "message", data: { sequence: 1, updated_sequence: 2, role: "user", body: "rename it", seen_at: "2026-07-24T12:05:00Z" } }],
      thread_total: 1,
      thread_last_sequence: 2,
    });
    expect(merged.items).toHaveLength(1);
    expect(merged.items[0].data.seen_at).toBe("2026-07-24T12:05:00Z");
    // The next cursor moves past the mutation bump so the bridge stops
    // re-shipping the same item on every poll.
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 2 });
    // End-to-end regression guard: the re-rendered thread shows Seen.
    const html = threadHtml(merged);
    expect(html).toContain("Seen");
    expect(html).not.toContain("Unread");
  });

  it("a zero-item delta leaves the accumulated items intact", () => {
    const cache = createThreadCache();
    const first = cache.absorb({ items: [item(1, "a"), item(2, "b")] });
    const second = cache.absorb({ items: [], thread_total: 2, thread_last_sequence: 2 });
    expect(second.items).toEqual(first.items);
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 2 });
  });

  it("resets to a full refetch when thread_total disagrees with what it holds", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a"), item(2, "b"), item(3, "c")] });
    // The bridge restarted (or the entity swapped): it now reports fewer items
    // than we hold. The cache drops its state so the next poll refetches whole.
    cache.absorb({ items: [], thread_total: 1, thread_last_sequence: 1 });
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
    const refetched = cache.absorb({ items: [item(1, "only")] });
    expect(refetched.items.map((i) => i.data.sequence)).toEqual([1]);
  });

  it("passes a missing thread through and clears its state", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a")] });
    expect(cache.absorb(null)).toBeNull();
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
  });
});

describe("thread cache paging (the window over a long conversation)", () => {
  const item = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

  it("names the page it can hold on a first load, so the daemon knows to bound one", () => {
    const cache = createThreadCache();
    // A daemon that hears no bound answers with the conversation whole, which
    // is the only answer a client that cannot page can reconcile. Asking is
    // what makes the answer a window.
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
  });
  // What the daemon's `thread.page` ships: the newest items it was asked for,
  // plus the whole conversation's size and the seek for the page above.
  const page = (items, { thread_total, has_more }) => ({
    id: "thread:run-1",
    items,
    revisions: [],
    thread_total,
    thread_last_sequence: items.at(-1)?.data.sequence || 0,
    oldest_sequence: items[0]?.data.sequence ?? null,
    has_more,
  });

  it("holds a bounded first page without calling it a loss", () => {
    const cache = createThreadCache();
    const opened = cache.absorb(page([item(98, "y"), item(99, "z")], { thread_total: 99, has_more: true }));
    expect(opened.items.map((i) => i.data.sequence)).toEqual([98, 99]);
    // The window is 2 of 99 on purpose. A cursor here is the whole point of
    // paging: dropping to a full refetch would ship the other 97 every tick.
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 99 });
    const polled = cache.absorb({ items: [item(100, "new")], thread_total: 100, thread_last_sequence: 100 });
    expect(polled.items.map((i) => i.data.sequence)).toEqual([98, 99, 100]);
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 100 });
  });

  it("resets when the window no longer reaches the newest item the daemon names", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(98, "y"), item(99, "z")], { thread_total: 99, has_more: true }));
    // A delta went missing: the daemon says 101 is the newest and shipped
    // nothing that gets us there, so what we hold has a hole in it.
    const gapped = cache.absorb({ items: [], thread_total: 101, thread_last_sequence: 101 });
    expect(gapped.items.map((i) => i.data.sequence)).toEqual([98, 99]);
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
  });

  it("resets when the conversation holds fewer items than the window does", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(1, "a"), item(2, "b"), item(3, "c")], { thread_total: 3, has_more: false }));
    // A bridge restart, or another entity's conversation under the same id:
    // the sequences still line up at the top but the whole is smaller than
    // the part we hold.
    cache.absorb({ items: [], thread_total: 2, thread_last_sequence: 3 });
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
  });

  it("cannot open a window on a delta, so a repaint after a reset still refetches", () => {
    const cache = createThreadCache();
    // The reader has paged all the way back: the window IS the conversation.
    cache.absorb(page([item(1, "a"), item(2, "b"), item(3, "c")], { thread_total: 3, has_more: false }));

    // One poll interval later: a doc comment was deleted (an item removed, no
    // sequence spent) and the agent posted. The delta names a newest of 4 and
    // a whole of 3, which is smaller than the four items the window would then
    // hold — so the cache renders what it has and drops itself for a refetch.
    const delta = { items: [item(4, "d")], thread_total: 3, thread_last_sequence: 4 };
    expect(cache.absorb(delta).items.map((i) => i.data.sequence)).toEqual([1, 2, 3, 4]);
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });

    // A repaint before the next poll folds the SAME payload back through the
    // emptied cache — pressing a bubble, or leaving the chat and coming back,
    // is enough. It is a delta, and a delta says nothing about how far back the
    // conversation goes: taking it as the window would leave the reader with a
    // one-message thread, a cursor past the end of it, and no page above — a
    // view no later poll ever brings the rest back to.
    const repainted = cache.absorb(delta);
    expect(repainted.items.map((i) => i.data.sequence)).toEqual([4]);
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
    expect(cache.olderPageParam()).toBeNull();

    // The refetch that cursor asks for is what paints, and it heals.
    const healed = cache.absorb(page([item(2, "b"), item(3, "c"), item(4, "d")], { thread_total: 3, has_more: false }));
    expect(healed.items.map((i) => i.data.sequence)).toEqual([2, 3, 4]);
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 4 });
  });

  it("asks for the page above the window while the daemon says there is one", () => {
    const cache = createThreadCache();
    expect(cache.olderPageParam()).toBeNull();
    cache.absorb(page([item(98, "y"), item(99, "z")], { thread_total: 99, has_more: true }));
    expect(cache.hasOlderItems()).toBe(true);
    expect(cache.olderPageParam()).toEqual({ before_sequence: 98 });
    cache.absorbOlderPage(page([item(96, "w"), item(97, "x")], { thread_total: 99, has_more: false }), { before_sequence: 98 });
    expect(cache.hasOlderItems()).toBe(false);
    expect(cache.olderPageParam()).toEqual({ before_sequence: 96 });
  });

  it("never asks for older items when the first page already holds the start", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(1, "a"), item(2, "b")], { thread_total: 2, has_more: false }));
    expect(cache.hasOlderItems()).toBe(false);
  });

  it("folds an older page in at the front, in order, without disturbing the cursor", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(8, "h"), item(9, "i")], { thread_total: 9, has_more: true }));
    const older = page([item(5, "e"), item(6, "f"), item(7, "g")], { thread_total: 9, has_more: true });
    const widened = cache.absorbOlderPage(older, cache.olderPageParam());
    expect(widened.items.map((i) => i.data.sequence)).toEqual([5, 6, 7, 8, 9]);
    expect(older.items.map((i) => i.data.sequence)).toEqual([5, 6, 7]); // payload untouched
    // Older items arriving must not walk the forward cursor backwards: the
    // next poll still wants only what is newer than the newest we hold.
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 9 });
  });

  it("keeps the widened window through the next poll rather than resetting on it", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(8, "h"), item(9, "i")], { thread_total: 9, has_more: true }));
    cache.absorbOlderPage(page([item(6, "f"), item(7, "g")], { thread_total: 9, has_more: true }), cache.olderPageParam());
    const polled = cache.absorb({ items: [item(10, "j")], thread_total: 10, thread_last_sequence: 10 });
    expect(polled.items.map((i) => i.data.sequence)).toEqual([6, 7, 8, 9, 10]);
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 10 });
  });

  it("ignores an older page when there is no window left to extend", () => {
    const cache = createThreadCache();
    // The reader switched agents while the page was in flight, so the cache it
    // would extend is gone. Folding it in would make a window whose top is not
    // the conversation's newest — a hole, dressed as history.
    expect(cache.absorbOlderPage(page([item(1, "a")], { thread_total: 9, has_more: false }), { before_sequence: 2 })).toBeNull();
    expect(cache.cursorParam()).toStrictEqual({ thread_limit: FIRST_PAGE_ITEMS });
    expect(cache.olderPageParam()).toBeNull();
  });

  it("ignores an older page whose window was replaced while it was in flight", () => {
    const cache = createThreadCache();
    const newest = [];
    for (let sequence = 191; sequence <= 250; sequence += 1) newest.push(item(sequence, `m${sequence}`));
    cache.absorb(page(newest, { thread_total: 250, has_more: true }));
    const widened = [];
    for (let sequence = 131; sequence <= 190; sequence += 1) widened.push(item(sequence, `m${sequence}`));
    const seek = cache.olderPageParam();
    cache.absorbOlderPage(page(widened, { thread_total: 250, has_more: true }), seek);

    // The reader scrolls back past 131 and the page for it goes out. While it
    // is in flight the poll trips the gap check and drops the cache, and the
    // poll after that opens a fresh window on the newest items.
    const staleSeek = cache.olderPageParam();
    expect(staleSeek).toEqual({ before_sequence: 131 });
    cache.absorb({ items: [], thread_total: 250, thread_last_sequence: 999 });
    cache.absorb(page(newest, { thread_total: 250, has_more: true }));

    // The page now lands under a window it was never above. Taking it would
    // seat 71..130 directly under 191..250 with sixty items missing between
    // them — and leave the floor at 71, so every further scroll back walks
    // downward and 131..190 could never be asked for again.
    const stale = [];
    for (let sequence = 71; sequence <= 130; sequence += 1) stale.push(item(sequence, `m${sequence}`));
    expect(cache.absorbOlderPage(page(stale, { thread_total: 250, has_more: true }), staleSeek)).toBeNull();
    expect(cache.olderPageParam()).toEqual({ before_sequence: 191 });
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 250 });
  });

  it("keeps the top the reader reached when the page it opened on is absorbed again", () => {
    const cache = createThreadCache();
    // Every repaint folds the payload in hand back through the cache, and the
    // payload in hand stays the page the window was opened on until the next
    // poll replaces it with a delta. That page says there is more above ITS
    // floor, which stopped being the window's floor the moment the reader
    // scrolled back to the start.
    const openedOn = page([item(8, "h"), item(9, "i")], { thread_total: 9, has_more: true });
    cache.absorb(openedOn);
    cache.absorbOlderPage(page([item(1, "a"), item(2, "b"), item(3, "c"), item(4, "d"), item(5, "e"), item(6, "f"), item(7, "g")], { thread_total: 9, has_more: false }), cache.olderPageParam());
    expect(cache.hasOlderItems()).toBe(false);

    cache.absorb(openedOn);

    // Believing it again would put the reader back at a top they have already
    // reached, and every further scroll gesture would ask the daemon for a
    // page it has already said does not exist.
    expect(cache.hasOlderItems()).toBe(false);
  });

  it("forgets that older items remain when it resets", () => {
    const cache = createThreadCache();
    cache.absorb(page([item(9, "i")], { thread_total: 9, has_more: true }));
    cache.reset();
    expect(cache.hasOlderItems()).toBe(false);
  });

  it("keeps an item mutated below the window out of it, and still moves past the bump", () => {
    const cache = createThreadCache();
    const opened = [];
    for (let sequence = 191; sequence <= 250; sequence += 1) opened.push(item(sequence, `m${sequence}`));
    cache.absorb(page(opened, { thread_total: 250, has_more: true }));

    // The agent resolved a doc comment made near the start of the
    // conversation: item 5 is stamped and its updated_sequence bumped to the
    // newest the daemon has, without a single item being appended. The forward
    // cursor selects on that bump, so the delta ships item 5 alone — from 186
    // items below the window's floor.
    const delta = cache.absorb({
      items: [{ type: "message", data: { sequence: 5, updated_sequence: 251, role: "user", body: "rename it", resolved_by_revision: "rev-2" } }],
      thread_total: 250,
      thread_last_sequence: 251,
    });

    // Taking it would seat message 5 directly above message 191 with 185
    // messages missing between them, and leave the window's floor at 5 — so
    // one scroll back would answer with items 1..4, say there is no more, and
    // bury the rest of the conversation for the life of the view.
    expect(delta.items.map((i) => i.data.sequence)).toEqual(opened.map((i) => i.data.sequence));
    expect(cache.olderPageParam()).toEqual({ before_sequence: 191 });
    expect(cache.hasOlderItems()).toBe(true);
    // The bump is still accounted for: a cursor left at 250 would have the
    // daemon re-ship item 5 on every poll for as long as the view is open.
    expect(cache.cursorParam()).toEqual({ thread_after_sequence: 251 });

    const older = [];
    for (let sequence = 131; sequence <= 190; sequence += 1) older.push(item(sequence, `m${sequence}`));
    const widened = cache.absorbOlderPage(page(older, { thread_total: 250, has_more: true }), cache.olderPageParam());
    expect(widened.items[0].data.sequence).toBe(131);
    expect(widened.items.map((i) => i.data.sequence)).toEqual(
      [...older, ...opened].map((i) => i.data.sequence),
    );
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

  it("escapes an attachment name rather than rendering it", () => {
    const html = threadHtml(withAttachments([
      { name: '<img src=x onerror="boom">.png', path: ".build/attachments/x.png", mime: "image/png", size: 1 },
    ]));
    expect(html).not.toContain("onerror=\"boom\"");
    expect(html).toContain("&lt;img");
  });

  it("fills an inline image from the bytes the bridge hands back, once", async () => {
    document.body.innerHTML = `<div id="host">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]))}</div>`;
    const host = document.querySelector("#host");
    const asked = [];
    const load = (path) => {
      asked.push(path);
      return Promise.resolve({ mime: "image/png", content_b64: "AAAA" });
    };

    wireThreadAttachments(host, load);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(host.querySelector("img.thread-attachment-image").getAttribute("src")).toBe("data:image/png;base64,AAAA");

    // A polling surface re-renders the timeline constantly; the bytes are
    // content-addressed and immutable, so asking twice is pure waste.
    document.body.innerHTML = `<div id="host">${threadHtml(withAttachments([
      { name: "shot.png", path: ".build/attachments/ab12-shot.png", mime: "image/png", size: 4 },
    ]))}</div>`;
    wireThreadAttachments(document.querySelector("#host"), load);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(asked).toEqual([".build/attachments/ab12-shot.png"]);
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
  const mountWithTray = (overrides = {}) => {
    document.body.innerHTML = `<div id="host">${composerHtml({
      inputId: "ti",
      sendId: "ts",
      hintId: "th",
      placeholder: "Say something…",
      attachable: true,
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
