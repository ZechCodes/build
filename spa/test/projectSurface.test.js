// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { projectSurfaceTabs } from "../src/views/mainWorktree.js";

describe("project surface chrome", () => {
  // Conversation leads, as it does on every other worktree surface: the primary
  // checkout has an owner-backed thread of its own.
  it("puts project panes first, then the checkout's agent and terminal tabs", () => {
    expect(projectSurfaceTabs([{ id: "term-1", label: "Terminal 1", closable: true }])).toEqual([
      { id: "conversation", label: "Conversation" },
      { id: "inbox", label: "Inbox" },
      { id: "issues", label: "Issues" },
      { id: "changes", label: "Changes" },
      { id: "files", label: "Files" },
      { id: "archive", label: "Archive" },
      { id: "agent", label: "Agent" },
      { id: "term-1", label: "Terminal 1", closable: true },
    ]);
  });

});
