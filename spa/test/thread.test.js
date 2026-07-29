// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { createThreadCache, currentRevisionId, threadHtml, wireThreadComposer, wireThreadRevisionLinks } from "../src/core/thread.js";
import { planThreadMessages, diffThreadMessages } from "../src/core/notes.js";

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
    expect(document.querySelector("time").textContent).toContain("Jul 24, 2026");
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

  it("keeps done as status and renders one concise requested/done message", () => {
    const html = threadHtml(
      {
        items: [{ type: "event", data: { event: "done", summary: "Implemented persistent review conversations.\n- Changed four files\n- Ran every test" } }],
        last_completion: {
          critical_files: ["src/plan.js"],
          risk_notes: ["Keep the durable thread intact."],
          decisions: [],
          skips: [],
        },
      },
      { initialMessage: "Make review conversations persistent\n- include every implementation detail", composer: true },
    );

    document.body.innerHTML = html;
    const messages = [...document.querySelectorAll(".thread-message")];
    expect(messages[0].classList.contains("user")).toBe(true);
    expect(messages[0].textContent).toContain("Make review conversations persistent");
    expect(messages.at(-1).classList.contains("agent")).toBe(true);
    expect(messages.at(-1).textContent).toContain("Requested");
    expect(messages.at(-1).textContent).toContain("Make review conversations persistent");
    expect(messages.at(-1).textContent).not.toContain("include every implementation detail");
    expect(messages.at(-1).textContent).toContain("Done");
    expect(messages.at(-1).textContent).toContain("Implemented persistent review conversations.");
    expect(messages.at(-1).textContent).not.toContain("Changed four files");
    expect(messages.at(-1).textContent).not.toContain("Critical files");
    expect(messages.at(-1).textContent).not.toContain("src/plan.js");
    expect(document.querySelector(".thread-event").textContent).toContain("Agent reported done");
    expect(document.querySelector(".thread-event-detail")).toBeNull();
    expect(document.querySelector("details")).toBeNull();
    expect(document.querySelector("#planthreadinput")).not.toBeNull();
    expect(document.querySelector("#planthreadsend").textContent).toBe("Send");
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
    expect(document.querySelector("#diffthreadhint")).not.toBeNull();
    expect(document.querySelector("#diffthreadsend").textContent).toBe("Send");
  });

  it("does not duplicate a sequenced completion message with the legacy fallback", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "done", summary: "Implemented the requested change." } },
        { type: "message", data: { role: "agent", source: "completion", body: "Completion report\n\nCritical files\n- src/app.rs" } },
      ],
      last_completion: { critical_files: ["src/app.rs"], risk_notes: [], decisions: [], skips: [] },
    }, { initialMessage: "Fix the bug" });
    expect(document.querySelectorAll(".thread-completion")).toHaveLength(1);
    expect(document.querySelector(".thread-completion").textContent).not.toContain("src/app.rs");
  });

  it("identifies the harness that reported completion", () => {
    document.body.innerHTML = threadHtml({
      sessions: [{ provider: "Codex CLI" }],
      items: [
        { type: "event", data: { event: "session_started" } },
        { type: "event", data: { event: "done", summary: "Finished the task." } },
        { type: "message", data: { role: "agent", source: "completion", body: "Done" } },
      ],
    }, { initialMessage: "Do the task" });
    const completionHead = document.querySelector(".thread-completion .thread-message-head").textContent;
    expect(completionHead).toContain("Codex completed the request");
    expect(completionHead).not.toContain("Agent completed the request");
    expect(document.querySelector(".thread-items").textContent).toContain("Codex session started");
    expect(document.querySelector(".thread-items").textContent).toContain("Codex reported done");
  });
});

describe("thread cache (cursor merge for the detail polls)", () => {
  const item = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

  it("starts with a full fetch, then sends the last-known sequence as the cursor", () => {
    const cache = createThreadCache();
    expect(cache.cursorParam()).toEqual({});
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
    expect(cache.cursorParam()).toEqual({});
    const refetched = cache.absorb({ items: [item(1, "only")] });
    expect(refetched.items.map((i) => i.data.sequence)).toEqual([1]);
  });

  it("passes a missing thread through and clears its state", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a")] });
    expect(cache.absorb(null)).toBeNull();
    expect(cache.cursorParam()).toEqual({});
  });
});

describe("structured review messages", () => {
  it("preserves plan and diff anchors instead of flattening them into a prompt", () => {
    expect(planThreadMessages([{ snippet: "old plan", comment: "be concrete" }], "", "plan-r1", ".build/plan.md"))
      .toEqual([{ body: "be concrete", anchor: { artifact: "plan", revision_id: "plan-r1", path: ".build/plan.md", heading_path: [], snippet: "old plan" } }]);
    expect(diffThreadMessages([{ file: "src/a.js", lnA: 2, lnB: 4, snippet: "old()", comment: "rename" }], "ship safely", "diff-r1"))
      .toEqual([
        { body: "rename", anchor: { artifact: "diff", revision_id: "diff-r1", path: "src/a.js", side: "new", line_start: 2, line_end: 4, heading_path: [], snippet: "old()" } },
        { body: "ship safely", anchor: null },
      ]);
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
