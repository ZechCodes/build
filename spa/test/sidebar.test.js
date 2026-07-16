import { describe, it, expect } from "vitest";
import { buildSidebarModel, projectHtml, sidebarHtml } from "../src/core/sidebar.js";

const NOW = Date.parse("2026-07-11T12:00:00Z");
const iso = (hoursAgo) => new Date(NOW - hoursAgo * 3600 * 1000).toISOString();

const projects = [
  { project_id: "p1", name: "relaydb" },
  { project_id: "p2", name: "dotfiles" },
];

const task = (over) => ({
  task_id: "t1",
  project_id: "p1",
  goal: "Fix the thing",
  state: "review",
  needs_attention: true,
  stat: { files_changed: 2, insertions: 42, deletions: 26 },
  updated_at: iso(1),
  ...over,
});

const ui = () => ({ closed: new Set(), wtOpen: new Set(), activeTaskId: null });

describe("buildSidebarModel", () => {
  it("groups tasks by project into needs-you / running / done-recently", () => {
    const tasks = [
      task({ task_id: "a", state: "review", needs_attention: true }),
      task({ task_id: "b", state: "planning", needs_attention: false }),
      task({ task_id: "c", state: "merged", needs_attention: false, updated_at: iso(5) }),
      task({ task_id: "elsewhere", project_id: "p2", state: "building", needs_attention: false }),
    ];
    const [p1, p2] = buildSidebarModel({ projects, tasks, externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.needsYou.map((t) => t.task_id)).toEqual(["a"]);
    expect(p1.running.map((t) => t.task_id)).toEqual(["b"]);
    expect(p1.doneRecently.map((t) => t.task_id)).toEqual(["c"]);
    expect(p2.running.map((t) => t.task_id)).toEqual(["elsewhere"]);
  });

  it("unread counts needs-you tasks not yet read", () => {
    const tasks = [
      task({ task_id: "a" }),
      task({ task_id: "b" }),
      task({ task_id: "c", needs_attention: false, state: "building" }),
    ];
    const [p1] = buildSidebarModel({ projects, tasks, externalWorktrees: [], readIds: new Set(["a"]), nowMs: NOW });
    expect(p1.unread).toBe(1);
  });

  it("done-recently is capped at 3, newest first, and windowed to 7 days", () => {
    const tasks = [4, 1, 30 * 24, 2, 3].map((h, i) =>
      task({ task_id: `d${i}`, state: "merged", needs_attention: false, updated_at: iso(h) })
    );
    const [p1] = buildSidebarModel({ projects, tasks, externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.doneRecently.map((t) => t.task_id)).toEqual(["d1", "d3", "d4"]); // 1h, 2h, 3h — 30d dropped, capped at 3
  });

  it("counts worktrees and uncommitted per project", () => {
    const wts = [
      { worktree_id: "w1", project_id: "p1", branch: "feat-a", dirty_files: 2 },
      { worktree_id: "w2", project_id: "p1", branch: "feat-b", dirty_files: 0 },
      { worktree_id: "w3", project_id: "p2", branch: "other", dirty_files: 1 },
    ];
    const [p1, p2] = buildSidebarModel({ projects, tasks: [], externalWorktrees: wts, readIds: new Set(), nowMs: NOW });
    expect(p1.worktrees.length).toBe(2);
    expect(p1.uncommitted).toBe(1);
    expect(p2.uncommitted).toBe(1);
  });

  it("attaches the primary-changes summary per project (null when absent)", () => {
    const primaryChanges = [{ project_id: "p1", branch: "main", files_changed: 3, insertions: 12, deletions: 4 }];
    const [p1, p2] = buildSidebarModel({ projects, tasks: [], externalWorktrees: [], primaryChanges, readIds: new Set(), nowMs: NOW });
    expect(p1.primary).toEqual({ branch: "main", files_changed: 3, insertions: 12, deletions: 4 });
    expect(p2.primary).toBeNull();
  });
});

describe("projectHtml", () => {
  const model = (over = {}) => ({
    project_id: "p1",
    name: "relaydb",
    unread: 1,
    needsYou: [task({ task_id: "a" })],
    running: [task({ task_id: "b", state: "planning", needs_attention: false, stat: null, last_error: null })],
    doneRecently: [task({ task_id: "c", state: "merged", age_s: 5 * 3600 })],
    worktrees: [{ worktree_id: "w1", project_id: "p1", branch: "feat-a", dirty_files: 1 }],
    uncommitted: 1,
    ...over,
  });

  it("renders sections, badge, diffstat, age, and the worktree line", () => {
    const html = projectHtml(model(), ui());
    expect(html).toContain("relaydb");
    expect(html).toContain('class="badge sbadge">1<');
    expect(html).toContain("+42");
    expect(html).toContain("-26");
    expect(html).toContain("planning"); // running task with no stat shows its state
    expect(html).toContain("5h ago");
    expect(html).toContain("1 worktree");
    expect(html).toContain("1 uncommitted");
  });

  it("collapsed projects render only the header", () => {
    const u = ui();
    u.closed.add("p1");
    const html = projectHtml(model(), u);
    expect(html).toContain("relaydb");
    expect(html).not.toContain("Needs you");
    expect(html).toContain("▸");
  });

  it("escapes external strings (goals, branch names, project names)", () => {
    const m = model({
      name: "<img src=x>",
      needsYou: [task({ task_id: "a", goal: "<script>alert(1)</script>" })],
      worktrees: [{ worktree_id: "w1", project_id: "p1", branch: "<b>evil</b>", dirty_files: 0 }],
    });
    const u = ui();
    u.wtOpen.add("p1");
    const html = projectHtml(m, u);
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>evil</b>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("renders a main row with the branch and a dirty count, wired to the main surface", () => {
    const html = projectHtml(model({ worktrees: [], uncommitted: 0, primary: { branch: "main", files_changed: 2, insertions: 5, deletions: 1 } }), ui());
    expect(html).toContain('data-main="p1"');
    expect(html).toContain("main ");
    expect(html).toContain("2 uncommitted");
  });

  it("omits the dirty count when the main checkout is clean and the main row when unknown", () => {
    const clean = projectHtml(model({ worktrees: [], uncommitted: 0, primary: { branch: "trunk", files_changed: 0, insertions: 0, deletions: 0 } }), ui());
    expect(clean).toContain("data-main=");
    expect(clean).not.toContain("uncommitted");
    const none = projectHtml(model({ worktrees: [], uncommitted: 0, primary: null }), ui());
    expect(none).not.toContain("data-main=");
  });

  it("marks the route's active task", () => {
    const u = ui();
    u.activeTaskId = "a";
    expect(projectHtml(model(), u)).toContain('class="srow attn active"');
  });

  it("splits the header into a chevron toggle and a name that opens the project", () => {
    const html = projectHtml(model(), ui());
    expect(html).toContain('data-chev="p1"');
    expect(html).toContain('data-open="p1"');
    expect(html).toContain("sfolder"); // folder icon, not a box glyph
    expect(html).not.toContain("▣");
  });

  it("highlights the project whose page is open", () => {
    const u = ui();
    u.activeProjectId = "p1";
    expect(projectHtml(model(), u)).toContain("sproj-head active");
  });

  it("blocked tasks in needs-you carry the warning icon", () => {
    const m = model({ needsYou: [task({ task_id: "a", state: "blocked" })] });
    expect(projectHtml(m, ui())).toContain("▲");
  });
});

describe("sidebarHtml", () => {
  it("renders the header actions and an empty state", () => {
    const html = sidebarHtml([], ui());
    expect(html).toContain('id="side-add"');
    // the collapse toggle lives in the static shell (index.html), not the rail
    expect(html).not.toContain('id="side-collapse"');
    expect(html).toContain("No projects yet");
  });
});
