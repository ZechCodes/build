import { describe, expect, it } from "vitest";
import { workspaceKey } from "../src/core/deviceKey.js";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { activeEntryKey, inboxRowHtml, workspaceEntries } from "../src/core/inbox.js";

const DEVICE = "dev-1";

// The inbox never sees a bare row: every workspace, project and board item it
// folds has been through one machine's snapshot, which stamps the device and
// mints the account-wide keys (core/deviceKey.js). The fixtures go through the
// same door, so the rows carry `deviceId`, `projectKey` and `workspaceKey`.
const entriesOf = (workspaces, items = []) => {
  const view = liveFeedSnapshot(
    { items },
    { projects: [{ id: "project-1", name: "Payments" }] },
    { workspaces },
    DEVICE,
  );
  return workspaceEntries(view.workspaces, view.projects, view.items);
};

describe("workspace inbox rows", () => {
  it("shows the owning conversation's unread and running state without matching branch names", () => {
    const workspaces = [
      { id: "run-1", project_id: "project-1", status: "ready" },
      { id: "run-2", project_id: "project-1", status: "ready" },
      { id: "run-1", project_id: "other-project", status: "ready" },
    ];
    const items = [{ kind: "branch", project_id: "project-1", run_id: "run-1", branch: "main", unread: true, unread_count: 3, working: true }];
    const [owned, unrelated, otherProject] = entriesOf(workspaces, items);
    expect(owned).toMatchObject({ entityId: "run-1", unreadCount: 3, state: "unread", working: true });
    expect(unrelated).toMatchObject({ entityId: null, unreadCount: 0, working: false });
    expect(otherProject).toMatchObject({ entityId: null, unreadCount: 0, working: false });
  });

  it("reconnects an adopted workspace alias to its exact checkout's conversation", () => {
    const [entry] = entriesOf([{ id: "external-1", project_id: "project-1", root: "/work/checkout" }], [
      { kind: "branch", project_id: "project-1", run_id: "run-wrong", worktree_path: "/work/another", branch: "main", unread: true, unread_count: 9 },
      { kind: "branch", project_id: "project-1", run_id: "run-1", worktree_path: "/work/checkout", branch: "main", unread: true, unread_count: 2 },
    ]);
    expect(entry).toMatchObject({ entityId: "run-1", unreadCount: 2, state: "unread" });
  });

  it("opens a workspace at its first Git directory and never renders a destructive action", () => {
    const [entry] = entriesOf([
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
    ]);

    expect(entry.route).toEqual({
      name: "workspace",
      deviceId: DEVICE,
      projectId: "project-1",
      workspaceId: "workspace-1",
      sourceId: "source-api",
      tab: "changes",
    });
    expect(entry.facts).toBe("3 pushes · +42 −7");
    // No destructive verb on a workspace row that is not clean: the Done
    // action is `data-done="…"` (the menu item) or `data-workspace-done`; the
    // always-present `data-done-error` slot is where a refusal is painted, not
    // a control, so the rule is stated against the action attributes.
    expect(inboxRowHtml(entry)).not.toMatch(/data-done=|data-workspace-done|branch\.finish|Delete/);
  });

  it("marks a workspace route active and supports a workspace with no directories", () => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", name: "Empty" }]);

    expect(entry.route).toEqual({
      name: "workspace",
      deviceId: DEVICE,
      projectId: "project-1",
      workspaceId: "workspace-1",
      tab: "changes",
    });
    expect(entry.facts).toBe("Work summary unavailable");
    expect(activeEntryKey(entry.route, [entry])).toBe(`workspace:${workspaceKey(DEVICE, "workspace-1")}`);
  });

  it("does not turn an unknown workspace work summary into zero work", () => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", work_summary: null }]);
    expect(entry.facts).toBe("Work summary unavailable");
    const [partial] = entriesOf([{ id: "workspace-2", project_id: "project-1", work_summary: { pushes: 1 } }]);
    expect(partial.facts).toBe("Work summary unavailable");
    for (const invalid of ["1", -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      const [malformed] = entriesOf([
        { id: "workspace-3", project_id: "project-1", work_summary: { pushes: invalid, additions: 0, deletions: 0 } },
      ]);
      expect(malformed.facts).toBe("Work summary unavailable");
    }
    const [clean] = entriesOf([
      { id: "workspace-4", project_id: "project-1", work_summary: { pushes: 0, additions: 0, deletions: 0 } },
    ]);
    expect(clean.facts).toBe("0 pushes · +0 −0");
  });

  // A checkout the bridge could not build has no work to summarize, and saying
  // the summary is unavailable reads as a hiccup in the reporting rather than
  // as the thing that went wrong. The row wears the word the switcher wears for
  // the same workspace, and the first line of the reason when there is one.
  it("says a workspace failed, with the first line of why, rather than an unavailable summary", () => {
    const [failed] = entriesOf([
      {
        id: "workspace-1",
        project_id: "project-1",
        status: "failed",
        directories: [
          { source_id: "api", is_git: true, status: "ready" },
          { source_id: "web", status: "failed", error: "failed to make directory '/src/.git/worktrees/web': directory exists\nhint: reuse it" },
        ],
      },
    ]);
    expect(failed.facts).toBe("Failed: failed to make directory '/src/.git/worktrees/web': directory exists");

    const [silent] = entriesOf([{ id: "workspace-2", project_id: "project-1", status: "failed" }]);
    expect(silent.facts).toBe("Failed");
  });
});
