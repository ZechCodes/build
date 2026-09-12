import { describe, expect, it } from "vitest";
import { activeEntryKey, inboxRowHtml, workspaceEntries } from "../src/core/inbox.js";

const projects = [{ id: "project-1", name: "Payments" }];

describe("workspace inbox rows", () => {
  it("shows the owning conversation's unread and running state without matching branch names", () => {
    const workspaces = [
      { id: "run-1", project_id: "project-1", status: "ready" },
      { id: "run-2", project_id: "project-1", status: "ready" },
      { id: "run-1", project_id: "other-project", status: "ready" },
    ];
    const items = [{ kind: "branch", project_id: "project-1", run_id: "run-1", branch: "main", unread: true, unread_count: 3, working: true }];
    const [owned, unrelated, otherProject] = workspaceEntries(workspaces, projects, items);
    expect(owned).toMatchObject({ entityId: "run-1", unreadCount: 3, state: "unread", working: true });
    expect(unrelated).toMatchObject({ entityId: null, unreadCount: 0, working: false });
    expect(otherProject).toMatchObject({ entityId: null, unreadCount: 0, working: false });
  });

  it("reconnects an adopted workspace alias to its exact checkout's conversation", () => {
    const [entry] = workspaceEntries([{ id: "external-1", project_id: "project-1", root: "/work/checkout" }], projects, [
      { kind: "branch", project_id: "project-1", run_id: "run-wrong", worktree_path: "/work/another", branch: "main", unread: true, unread_count: 9 },
      { kind: "branch", project_id: "project-1", run_id: "run-1", worktree_path: "/work/checkout", branch: "main", unread: true, unread_count: 2 },
    ]);
    expect(entry).toMatchObject({ entityId: "run-1", unreadCount: 2, state: "unread" });
  });

  it("opens a workspace at its first Git directory and never renders a destructive action", () => {
    const [entry] = workspaceEntries(
      [
        {
          id: "workspace-1",
          project_id: "project-1",
          name: "Checkout",
          root: "/work/checkout",
          status: "active",
          work_summary: { pushes: 3, additions: 42, deletions: 7 },
          directories: [
            { id: "docs", source_id: "source-docs", is_git: false },
            { id: "api", source_id: "source-api", is_git: true },
          ],
        },
      ],
      projects,
    );

    expect(entry.route).toEqual({
      name: "workspace",
      projectId: "project-1",
      workspaceId: "workspace-1",
      sourceId: "source-api",
      tab: "changes",
    });
    expect(entry.facts).toBe("3 pushes · +42 −7");
    expect(inboxRowHtml(entry)).not.toMatch(/data-done|branch\.finish|Delete/);
  });

  it("marks a workspace route active and supports a workspace with no directories", () => {
    const [entry] = workspaceEntries([{ id: "workspace-1", project_id: "project-1", name: "Empty" }], projects);

    expect(entry.route).toEqual({
      name: "workspace",
      projectId: "project-1",
      workspaceId: "workspace-1",
      tab: "changes",
    });
    expect(entry.facts).toBe("Work summary unavailable");
    expect(activeEntryKey(entry.route, [entry])).toBe("workspace:workspace-1");
  });

  it("does not turn an unknown workspace work summary into zero work", () => {
    const [entry] = workspaceEntries([{ id: "workspace-1", project_id: "project-1", work_summary: null }], projects);
    expect(entry.facts).toBe("Work summary unavailable");
    const [partial] = workspaceEntries([{ id: "workspace-2", project_id: "project-1", work_summary: { pushes: 1 } }], projects);
    expect(partial.facts).toBe("Work summary unavailable");
    for (const invalid of ["1", -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      const [malformed] = workspaceEntries([
        { id: "workspace-3", project_id: "project-1", work_summary: { pushes: invalid, additions: 0, deletions: 0 } },
      ], projects);
      expect(malformed.facts).toBe("Work summary unavailable");
    }
    const [clean] = workspaceEntries([
      { id: "workspace-4", project_id: "project-1", work_summary: { pushes: 0, additions: 0, deletions: 0 } },
    ], projects);
    expect(clean.facts).toBe("0 pushes · +0 −0");
  });
});
