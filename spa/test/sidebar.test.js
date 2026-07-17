import { describe, it, expect } from "vitest";
import { buildSidebarModel, projectHtml, sidebarHtml } from "../src/core/sidebar.js";

const NOW = Date.parse("2026-07-11T12:00:00Z");
const iso = (hoursAgo) => new Date(NOW - hoursAgo * 3600 * 1000).toISOString();

const projects = [
  { project_id: "p1", name: "relaydb" },
  { project_id: "p2", name: "dotfiles" },
];

const run = (over) => ({
  run_id: "r1",
  project_id: "p1",
  goal: "Fix the thing",
  state: "review",
  needs_attention: true,
  stat: { files_changed: 2, insertions: 42, deletions: 26 },
  updated_at: iso(1),
  ...over,
});

const plan = (over) => ({
  plan_id: "pl1",
  project_id: "p1",
  goal: "Design the thing",
  state: "plan_review",
  needs_attention: true,
  updated_at: iso(1),
  ...over,
});

const ui = () => ({ closed: new Set(), wtOpen: new Set(), activeRunId: null });

describe("buildSidebarModel — runs", () => {
  it("groups runs by project into needs-you / running / done-recently", () => {
    const runs = [
      run({ run_id: "a", state: "review", needs_attention: true }),
      run({ run_id: "b", state: "building", needs_attention: false }),
      run({ run_id: "c", state: "merged", needs_attention: false, updated_at: iso(5) }),
      run({ run_id: "elsewhere", project_id: "p2", state: "building", needs_attention: false }),
    ];
    const [p1, p2] = buildSidebarModel({ projects, runs, plans: [], externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.needsYou.map((r) => r.run_id)).toEqual(["a"]);
    expect(p1.running.map((r) => r.run_id)).toEqual(["b"]);
    expect(p1.doneRecently.map((r) => r.run_id)).toEqual(["c"]);
    expect(p2.running.map((r) => r.run_id)).toEqual(["elsewhere"]);
  });

  it("archived runs are terminal — done-recently, never running or needs-you", () => {
    const runs = [run({ run_id: "x", state: "archived", needs_attention: false, updated_at: iso(5) })];
    const [p1] = buildSidebarModel({ projects, runs, plans: [], externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.doneRecently.map((r) => r.run_id)).toEqual(["x"]);
    expect(p1.running).toEqual([]);
    expect(p1.unread).toBe(0);
  });

  it("done-recently is capped at 3, newest first, and windowed to 7 days", () => {
    const runs = [4, 1, 30 * 24, 2, 3].map((h, i) =>
      run({ run_id: `d${i}`, state: "merged", needs_attention: false, updated_at: iso(h) })
    );
    const [p1] = buildSidebarModel({ projects, runs, plans: [], externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.doneRecently.map((r) => r.run_id)).toEqual(["d1", "d3", "d4"]); // 1h, 2h, 3h — 30d dropped, capped at 3
  });
});

describe("buildSidebarModel — plans", () => {
  it("groups plans by project into needs-you / approved / drafting / done", () => {
    const plans = [
      plan({ plan_id: "a", state: "plan_review", needs_attention: true }),
      plan({ plan_id: "b", state: "approved", needs_attention: false }),
      plan({ plan_id: "c", state: "drafting", needs_attention: false }),
      plan({ plan_id: "d", state: "abandoned", needs_attention: false, updated_at: iso(5) }),
      plan({ plan_id: "elsewhere", project_id: "p2", state: "drafting", needs_attention: false }),
    ];
    const [p1, p2] = buildSidebarModel({ projects, runs: [], plans, externalWorktrees: [], readIds: new Set(), nowMs: NOW });
    expect(p1.planNeedsYou.map((p) => p.plan_id)).toEqual(["a"]);
    expect(p1.planApproved.map((p) => p.plan_id)).toEqual(["b"]);
    expect(p1.planDrafting.map((p) => p.plan_id)).toEqual(["c"]);
    expect(p1.plansDone.map((p) => p.plan_id)).toEqual(["d"]);
    expect(p2.planDrafting.map((p) => p.plan_id)).toEqual(["elsewhere"]);
  });
});

describe("buildSidebarModel — unread and worktrees", () => {
  it("unread counts needs-you runs and plans not yet read", () => {
    const runs = [run({ run_id: "a" }), run({ run_id: "b" }), run({ run_id: "c", needs_attention: false, state: "building" })];
    const plans = [plan({ plan_id: "pa" }), plan({ plan_id: "pb", needs_attention: false, state: "approved" })];
    const [p1] = buildSidebarModel({ projects, runs, plans, externalWorktrees: [], readIds: new Set(["a"]), nowMs: NOW });
    expect(p1.unread).toBe(2); // b (run) + pa (plan); a is read, c/pb not needing attention
  });

  it("counts worktrees and uncommitted per project", () => {
    const wts = [
      { worktree_id: "w1", project_id: "p1", branch: "feat-a", dirty_files: 2 },
      { worktree_id: "w2", project_id: "p1", branch: "feat-b", dirty_files: 0 },
      { worktree_id: "w3", project_id: "p2", branch: "other", dirty_files: 1 },
    ];
    const [p1, p2] = buildSidebarModel({ projects, runs: [], plans: [], externalWorktrees: wts, readIds: new Set(), nowMs: NOW });
    expect(p1.worktrees.length).toBe(2);
    expect(p1.uncommitted).toBe(1);
    expect(p2.uncommitted).toBe(1);
  });

  it("attaches the primary-changes summary per project (null when absent)", () => {
    const primaryChanges = [{ project_id: "p1", branch: "main", files_changed: 3, insertions: 12, deletions: 4 }];
    const [p1, p2] = buildSidebarModel({ projects, runs: [], plans: [], externalWorktrees: [], primaryChanges, readIds: new Set(), nowMs: NOW });
    expect(p1.primary).toEqual({ branch: "main", files_changed: 3, insertions: 12, deletions: 4 });
    expect(p2.primary).toBeNull();
  });
});

describe("projectHtml", () => {
  const model = (over = {}) => ({
    project_id: "p1",
    name: "relaydb",
    unread: 1,
    needsYou: [run({ run_id: "a" })],
    running: [run({ run_id: "b", state: "building", needs_attention: false, stat: null, last_error: null })],
    doneRecently: [run({ run_id: "c", state: "merged", age_s: 5 * 3600 })],
    planNeedsYou: [],
    planApproved: [],
    planDrafting: [],
    plansDone: [],
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
    expect(html).toContain("building"); // running run with no stat shows its state
    expect(html).toContain("5h ago");
    expect(html).toContain("1 worktree");
    expect(html).toContain("1 uncommitted");
  });

  it("run rows are keyed by run_id and route to the run surface", () => {
    const html = projectHtml(model(), ui());
    expect(html).toContain('data-run="a"');
    expect(html).toContain('data-tab="changes"');
  });

  it("a stage_gate run row opens on Stages (defaultRunTab — matching the board/notifications)", () => {
    const m = model({ needsYou: [run({ run_id: "sg", state: "stage_gate" })], running: [], doneRecently: [] });
    const html = projectHtml(m, ui());
    expect(html).toContain('data-run="sg"');
    expect(html).toContain('data-tab="stages"');
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
      needsYou: [run({ run_id: "a", goal: "<script>alert(1)</script>" })],
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

  it("marks the route's active run", () => {
    const u = ui();
    u.activeRunId = "a";
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

  it("the main-checkout row highlights when its surface is the route", () => {
    const m = model({ primary: { branch: "main", files_changed: 0, insertions: 0, deletions: 0 } });
    const off = projectHtml(m, ui());
    expect(off).not.toContain("smain-line active");
    const u = ui();
    u.activeMainProjectId = "p1";
    expect(projectHtml(m, u)).toContain("smain-line active");
  });

  it("an open worktree row highlights when its surface is the route", () => {
    const u = ui();
    u.wtOpen = new Set(["p1"]);
    u.activeWorktreeId = "w1";
    const html = projectHtml(model(), u);
    expect(html).toContain("swt-item active");
    // The collapsed summary line takes the highlight instead when the list is closed.
    u.wtOpen = new Set();
    const closed = projectHtml(model(), u);
    expect(closed).not.toContain("swt-item active");
    expect(closed).toContain("swt-line active");
  });

  it("blocked runs in needs-you carry the warning icon", () => {
    const m = model({ needsYou: [run({ run_id: "a", state: "blocked" })] });
    expect(projectHtml(m, ui())).toContain("▲");
  });

  it("renders plan rows keyed by plan_id and routing to the plan cockpit (never a run)", () => {
    const m = model({
      needsYou: [],
      running: [],
      planNeedsYou: [plan({ plan_id: "pl-a", state: "plan_review" })],
      planApproved: [plan({ plan_id: "pl-b", state: "approved", needs_attention: false })],
      planDrafting: [plan({ plan_id: "pl-c", state: "drafting", needs_attention: false })],
    });
    const html = projectHtml(m, ui());
    expect(html).toContain("Plans");
    expect(html).toContain('data-plan="pl-a"');
    expect(html).toContain('data-plan="pl-b"');
    expect(html).toContain('data-plan="pl-c"');
    expect(html).toContain("Design the thing"); // the plan goal
    expect(html).not.toContain('data-run="pl-a"'); // plan rows never carry a run handle
  });

  it("marks the route's active plan", () => {
    const u = ui();
    u.activePlanId = "pl-a";
    const m = model({ needsYou: [], running: [], planNeedsYou: [plan({ plan_id: "pl-a", state: "plan_review" })] });
    expect(projectHtml(m, u)).toContain('class="srow splan attn active"');
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
