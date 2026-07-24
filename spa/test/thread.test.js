// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { currentRevisionId, threadHtml, wireThreadRevisionLinks } from "../src/core/thread.js";
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

  it("does not duplicate a sequenced completion message with the legacy fallback", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "message", data: { role: "agent", source: "completion", body: "Completion report\n\nCritical files\n- src/app.rs" } }],
      last_completion: { critical_files: ["src/app.rs"], risk_notes: [], decisions: [], skips: [] },
    });
    expect(document.querySelectorAll(".thread-completion")).toHaveLength(1);
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
