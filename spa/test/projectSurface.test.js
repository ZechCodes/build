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

  it("keeps both project creation actions in the persistent surface header", () => {
    const html = projectSurfaceActionsHtml();
    expect(html).toContain('id="newquick"');
    expect(html).toContain("Quick task");
    expect(html).toContain('id="newplan"');
    expect(html).toContain("New plan");
  });
});
