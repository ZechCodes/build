// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { createThreadCache, currentRevisionId, threadHtml, wireThreadRevisionLinks } from "../src/core/thread.js";
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

  it("starts with the plan prompt, renders completion as an agent message, and ends with a composer", () => {
    const html = threadHtml(
      {
        items: [{ type: "event", data: { event: "done", summary: "A detailed done report that must remain readable." } }],
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
    const messages = [...document.querySelectorAll(".thread-message")];
    expect(messages[0].classList.contains("user")).toBe(true);
    expect(messages[0].textContent).toContain("Make review conversations persistent");
    expect(messages.at(-1).classList.contains("agent")).toBe(true);
    expect(messages.at(-1).textContent).toContain("Completion report");
    expect(messages.at(-1).textContent).toContain("src/plan.js");
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
      items: [{ type: "message", data: { role: "agent", source: "completion", body: "Completion report\n\nCritical files\n- src/app.rs" } }],
      last_completion: { critical_files: ["src/app.rs"], risk_notes: [], decisions: [], skips: [] },
    });
    expect(document.querySelectorAll(".thread-completion")).toHaveLength(1);
  });

  it("identifies the harness that reported completion", () => {
    document.body.innerHTML = threadHtml({
      sessions: [{ provider: "Codex CLI" }],
      items: [
        { type: "event", data: { event: "session_started" } },
        { type: "event", data: { event: "done" } },
        { type: "message", data: { role: "agent", source: "completion", body: "Done" } },
      ],
    });
    expect(document.querySelector(".thread-message-head").textContent).toContain("Codex reported completion");
    expect(document.querySelector(".thread-message-head").textContent).not.toContain("Agent reported completion");
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

  it("de-duplicates a replayed sequence, keeping the item it already holds", () => {
    const cache = createThreadCache();
    cache.absorb({ items: [item(1, "a"), item(2, "held")] });
    const merged = cache.absorb({ items: [item(2, "replayed"), item(3, "c")], thread_total: 3, thread_last_sequence: 3 });
    expect(merged.items.map((i) => i.data.sequence)).toEqual([1, 2, 3]);
    expect(merged.items[1].data.body).toBe("held");
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
