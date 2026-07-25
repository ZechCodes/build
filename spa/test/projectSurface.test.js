// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { projectSurfaceActionsHtml, projectSurfaceTabs } from "../src/views/mainWorktree.js";

describe("project surface chrome", () => {
  it("puts Inbox first and keeps checkout and terminal tabs beside it", () => {
    expect(projectSurfaceTabs([{ id: "term-1", label: "Terminal 1", closable: true }])).toEqual([
      { id: "inbox", label: "Inbox" },
      { id: "changes", label: "Changes" },
      { id: "files", label: "Files" },
      { id: "term-1", label: "Terminal 1", closable: true },
    ]);
  });

  // Authoring a plan is the only way work enters Build; an unplanned coding
  // session is a claude/codex tab off the tab row's `+`, not a header action.
  it("keeps plan authoring as the one creation action in the surface header", () => {
    const html = projectSurfaceActionsHtml();
    expect(html).toContain('id="newplan"');
    expect(html).toContain("New plan");
    expect(html).not.toContain("newquick");
    expect(html).not.toContain("Quick task");
  });
});
