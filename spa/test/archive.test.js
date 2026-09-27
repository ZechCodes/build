// The account archive's pure model: what one list across every project says
// about work that has ended, and what its record shows when it is opened.

import { describe, it, expect } from "vitest";
import {
  archiveListHtml,
  archiveRecordHtml,
  archiveRowHtml,
  archiveRows,
} from "../src/core/archive.js";

const branch = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  title: "Fix the login flow",
  branch: "build/login",
  state: "archived",
  action: "delete",
  finished_at: "2026-08-10T09:30:00Z",
  run_id: "run-1",
  task_id: null,
  stages: null,
  worktree_id: "wt-1",
  worktree_path: "/wt/login",
  head_sha: "abc1234",
  upstream: "origin/build/login",
  unpushed: 0,
  dirty_files: 0,
  ...over,
});

const task = (over = {}) => ({
  kind: "task",
  project_id: "p2",
  project: "dotfiles",
  title: "Split the prompt templates",
  branch: null,
  state: "approved",
  action: null,
  finished_at: "2026-08-12T18:00:00Z",
  run_id: null,
  task_id: "task-1",
  stages: 3,
  worktree_id: null,
  worktree_path: null,
  head_sha: null,
  upstream: null,
  unpushed: null,
  dirty_files: null,
  ...over,
});

const workspace = (over = {}) => ({
  kind: "workspace",
  workspace_id: "workspace-1",
  project_id: "p1",
  project: "relaydb",
  title: "Clean checkout",
  state: "finished",
  finished_at: "2026-08-13T18:00:00Z",
  worktree_path: "/work/clean",
  ...over,
});

describe("archiveRows", () => {
  it("reads both kinds off the wire, newest first", () => {
    const rows = archiveRows({ items: [branch(), task()] });
    expect(rows.map((row) => row.kind)).toEqual(["task", "branch"]);
    expect(rows[0].title).toBe("Split the prompt templates");
    expect(rows[0].kindLabel).toBe("Task");
    expect(rows[1].kindLabel).toBe("Branch");
    expect(rows[1].project).toBe("relaydb");
  });

  it("reads finished workspaces as workspace archive rows", () => {
    const [row] = archiveRows({ items: [workspace()] });
    expect(row).toMatchObject({ key: "workspace-1", workspaceId: "workspace-1", kind: "workspace", kindLabel: "Workspace", stateLabel: "Finished" });
  });

  it("says how the work ended in words, and when", () => {
    const [merged, abandoned, unknown] = archiveRows({
      items: [
        branch({ state: "merged", finished_at: "2026-08-03T00:00:00Z" }),
        branch({ state: "abandoned", finished_at: "2026-08-02T00:00:00Z" }),
        branch({ state: "sideways", finished_at: "2026-08-01T00:00:00Z" }),
      ],
    });
    expect(merged.stateLabel).toBe("Merged");
    expect(abandoned.stateLabel).toBe("Abandoned");
    // A state this client has never heard of still reads as something.
    expect(unknown.stateLabel).toBe("sideways");
    expect(merged.finishedLabel).toMatch(/2026/);
  });

  it("keeps rows whose stamp or fields the bridge could not fill", () => {
    const rows = archiveRows({
      items: [null, "junk", branch({ finished_at: null, title: "", branch: "build/x" }), task({ title: null, task_id: null })],
    });
    expect(rows).toHaveLength(2);
    // A row with no stamp sorts last rather than jumping the queue.
    expect(rows[rows.length - 1].finishedLabel).toBe("date unknown");
    const named = rows.find((row) => row.kind === "branch");
    expect(named.title).toBe("build/x");
    const untitled = rows.find((row) => row.kind === "task");
    expect(untitled.title).toBe("(untitled)");
  });

  it("gives every row a key of its own, even without an id behind it", () => {
    const rows = archiveRows({
      items: [
        branch(),
        branch({ run_id: null, worktree_id: null }),
        branch({ run_id: null, worktree_id: null }),
      ],
    });
    expect(new Set(rows.map((row) => row.key)).size).toBe(3);
  });

  it("tolerates a payload that is not a list at all", () => {
    expect(archiveRows()).toEqual([]);
    expect(archiveRows({})).toEqual([]);
    expect(archiveRows({ items: "nope" })).toEqual([]);
  });
});

describe("archiveListHtml", () => {
  it("shows each row's title, project, kind, ending and date", () => {
    const rows = archiveRows({ items: [branch()] });
    const html = archiveListHtml(rows, {});
    expect(html).toContain("Fix the login flow");
    expect(html).toContain("relaydb");
    expect(html).toContain("Branch");
    expect(html).toContain("Archived");
    expect(html).toContain(rows[0].finishedLabel);
    expect(html).toContain(`data-key="${rows[0].key}"`);
  });

  it("says the archive is empty rather than showing nothing", () => {
    expect(archiveListHtml([], {})).toContain("Nothing is archived");
  });

  it("opens the selected row's record under it, and only that one", () => {
    const rows = archiveRows({ items: [branch(), task()] });
    const html = archiveListHtml(rows, { openKey: rows[0].key });
    expect(html).toContain("archive-record");
    expect(html.match(/archive-record/g)).toHaveLength(1);
    expect(html).toContain("Split the prompt templates");
  });

  it("escapes every name the repo chose", () => {
    const rows = archiveRows({ items: [branch({ title: '<img src=x onerror=1>', branch: '"><b>' })] });
    const html = archiveRowHtml(rows[0], { openKey: rows[0].key });
    expect(html).not.toContain("<img");
    expect(html).not.toContain('"><b>');
    expect(html).toContain("&lt;img");
  });
});

describe("archiveRecordHtml", () => {
  it("states a branch's record, read-only", () => {
    const [row] = archiveRows({ items: [branch()] });
    const html = archiveRecordHtml(row);
    expect(html).toContain("build/login");
    expect(html).toContain("/wt/login");
    expect(html).toContain("abc1234");
    expect(html).toContain("origin/build/login");
    expect(html).toContain("Checkout deleted");
    expect(html).not.toContain("<button");
  });

  it("states a task's record without pretending it had a checkout", () => {
    const [row] = archiveRows({ items: [task()] });
    const html = archiveRecordHtml(row);
    expect(html).toContain("3 stages");
    expect(html).not.toContain("HEAD");
    expect(html).not.toContain("Finish action");
  });

  it("leaves out what the bridge did not send", () => {
    const [row] = archiveRows({ items: [branch({ head_sha: null, upstream: null, action: null, worktree_path: null })] });
    const html = archiveRecordHtml(row);
    expect(html).not.toContain("HEAD");
    expect(html).not.toContain("Upstream");
    expect(html).not.toContain("Finish action");
    expect(html).toContain("build/login");
  });
});
