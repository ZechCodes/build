// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { currentRevisionId, threadHtml, wireThreadRevisionLinks } from "../src/core/thread.js";
import { planThreadMessages, diffThreadMessages } from "../src/core/notes.js";

describe("conversation thread rendering", () => {
  it("renders messages, zero-token events, seen state, and revision resolution", async () => {
    const thread = {
      revisions: [{ id: "diff-revision-2-abcd", artifact: "diff" }],
      items: [
        { type: "message", data: { role: "user", body: "rename this", seen_at: "now", resolved_by_revision: "diff-revision-2-abcd", anchor: { path: "src/a.js", line_start: 4, line_end: 4 } } },
        { type: "message", data: { role: "agent", body: "Which name?" } },
        { type: "event", data: { event: "revision_created", revision_id: "diff-revision-2-abcd" } },
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
    wireThreadRevisionLinks(document.body, async (revisionId) => ({ revision_id: revisionId, contents: "+renamed" }));
    document.querySelector(".thread-revision-link").click();
    await Promise.resolve();
    expect(document.querySelector(".thread-revision-view").textContent).toContain("+renamed");
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
