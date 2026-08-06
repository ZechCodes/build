// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { projectSurfaceTabs } from "../src/views/mainWorktree.js";

describe("project surface chrome", () => {
  // Conversation leads, as it does on every other worktree surface: the primary
  // checkout has an owner-backed thread of its own.
  it("puts project panes first, then the checkout's agent and terminal tabs", () => {
    expect(projectSurfaceTabs([{ id: "term-1", label: "Terminal 1", closable: true }])).toEqual([
      { id: "conversation", label: "Conversation" },
      { id: "issues", label: "Issues" },
      { id: "changes", label: "Changes" },
      { id: "files", label: "Files" },
      { id: "agent", label: "Agent" },
      { id: "term-1", label: "Terminal 1", closable: true },
    ]);
  });

  // Inbox and Archive are project-wide, not primary-checkout panes: they moved to
  // the tab bar's right cluster, which every project surface carries.
  it("leaves the project-wide entries to the shared right cluster", () => {
    const ids = projectSurfaceTabs([]).map((tab) => tab.id);
    expect(ids).not.toContain("inbox");
    expect(ids).not.toContain("archive");
  });
});
