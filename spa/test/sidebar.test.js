// The rail's rendering. WHICH entries appear and in what order is rail.js's job
// (and rail.test.js's); this covers the shape of a project block: the two header
// lines, the entry rows, and the Worktrees row beneath them.

import { describe, it, expect } from "vitest";
import { buildSidebarModel, projectHtml, sidebarHtml, checkoutStatusHtml } from "../src/core/sidebar.js";

const NOW = Date.parse("2026-07-28T12:00:00Z");
const iso = (hoursAgo) => new Date(NOW - hoursAgo * 3600 * 1000).toISOString();

const projects = [
  { project_id: "p1", name: "relaydb" },
  { project_id: "p2", name: "dotfiles" },
];

const run = (over) => ({
  run_id: "r1",
  project_id: "p1",
  goal: "Fix the thing",
  branch: "build/fix-the-thing",
  state: "review",
  needs_attention: true,
  stat: { files_changed: 2, insertions: 42, deletions: 26, ahead: 3, behind: 1 },
  state_changed_at: iso(1),
  updated_at: iso(1),
  attention: { resume_at: iso(2), interacted: true, seen: false },
  ...over,
});

const plan = (over) => ({
  plan_id: "pl1",
  project_id: "p1",
  goal: "Design the thing",
  state: "plan_review",
  needs_attention: true,
  state_changed_at: iso(1),
  updated_at: iso(1),
  attention: { resume_at: iso(3), interacted: true, seen: true },
  ...over,
});

const worktree = (over) => ({
  worktree_id: "w1",
  project_id: "p1",
  name: "spike",
  branch: "build/spike",
  agent_working: false,
  dirty_files: 0,
  ahead: 0,
  behind: 0,
  diffstat: { files_changed: 0, insertions: 0, deletions: 0 },
  attention: { resume_at: null, interacted: false, seen: false },
  ...over,
});

const model = (over = {}) =>
  buildSidebarModel({
    projects,
    runs: [run()],
    plans: [plan()],
    externalWorktrees: [],
    primaryChanges: [],
    readIds: new Set(),
    nowMs: NOW,
    ...over,
  })[0];

const ui = (over = {}) => ({
  closed: new Set(),
  wtOpen: new Set(),
  activeRunId: null,
  activePlanId: null,
  activeWorktreeId: null,
  activeProjectId: null,
  activeMainProjectId: null,
  ...over,
});

describe("buildSidebarModel", () => {
  it("keeps each project's own work", () => {
    const [p1, p2] = buildSidebarModel({
      projects,
      runs: [run(), run({ run_id: "elsewhere", project_id: "p2" })],
      plans: [],
      externalWorktrees: [],
      primaryChanges: [],
      readIds: new Set(),
      nowMs: NOW,
    });
    expect(p1.entries.map((e) => e.id)).toEqual(["r1"]);
    expect(p2.entries.map((e) => e.id)).toEqual(["elsewhere"]);
  });

  it("counts unread work-that-needs-you for the badge", () => {
    expect(model({ readIds: new Set(["r1"]) }).unread).toBe(1); // the plan is still unread
  });

  it("attaches the primary-changes summary, sync counts and all", () => {
    const m = model({
      primaryChanges: [
        { project_id: "p1", branch: "main", ahead: 2, behind: 0, files_changed: 3, insertions: 12, deletions: 4 },
      ],
    });
    expect(m.primary).toEqual({
      branch: "main",
      path: null,
      ahead: 2,
      behind: 0,
      files_changed: 3,
      insertions: 12,
      deletions: 4,
    });
  });

  it("hands worktrees the entries did not show to the Worktrees row", () => {
    const m = model({
      externalWorktrees: [
        worktree({ worktree_id: "shown", name: "shown", attention: { resume_at: iso(1), interacted: true, seen: true } }),
        worktree({ worktree_id: "hidden", name: "hidden" }),
      ],
    });
    expect(m.entries.map((e) => e.id)).toContain("shown");
    expect(m.worktrees.map((w) => w.id)).toEqual(["hidden"]);
  });
});

describe("a project block", () => {
  it("carries the name, then the checkout's branch and status", () => {
    const html = projectHtml(
      model({ primaryChanges: [{ project_id: "p1", branch: "main", ahead: 2, behind: 1, files_changed: 1, insertions: 5, deletions: 0 }] }),
      ui(),
    );
    expect(html).toContain("relaydb");
    expect(html).toContain('data-main="p1"');
    expect(html).toContain("↑2");
    expect(html).toContain("↓1");
    expect(html).toContain("+5");
  });

  it("keeps both header lines when collapsed, and drops the body", () => {
    const html = projectHtml(
      model({ primaryChanges: [{ project_id: "p1", branch: "main", files_changed: 0, insertions: 0, deletions: 0 }] }),
      ui({ closed: new Set(["p1"]) }),
    );
    expect(html).toContain("relaydb");
    expect(html).toContain("main");
    expect(html).not.toContain("sproj-body");
  });

  it("renders a row as dot, name, then floating git status", () => {
    const html = projectHtml(model(), ui());
    expect(html).toContain("sdot");
    expect(html).toContain("Fix the thing");
    expect(html).toContain("↑3");
    expect(html).toContain("+42");
    expect(html).toContain("-26");
  });

  // ↑2 against what? A tracking branch is measured against its upstream and an
  // untracked one against the base branch, so the row says which on hover.
  it("names what a worktree's ahead/behind was measured against", () => {
    const m = model({
      runs: [],
      plans: [],
      externalWorktrees: [
        worktree({
          ahead: 2,
          sync_base: "origin/build/spike",
          attention: { resume_at: iso(1), interacted: true, seen: true },
        }),
      ],
    });
    expect(projectHtml(m, ui())).toContain('title="↑2 ahead of origin/build/spike"');
  });

  it("falls back to the branch when a row has no name", () => {
    const m = model({
      runs: [],
      plans: [],
      externalWorktrees: [
        worktree({ name: "", branch: "build/nameless", attention: { resume_at: iso(1), interacted: true, seen: true } }),
      ],
    });
    expect(projectHtml(m, ui())).toContain("build/nameless");
  });

  it("pulses a working entry, warns on an unseen finished one, settles a seen one", () => {
    expect(projectHtml(model({ runs: [run({ state: "building" })], plans: [] }), ui())).toContain("sdot-working");
    expect(projectHtml(model({ runs: [run({ state: "review" })], plans: [] }), ui())).toContain("sdot-unseen");
    const seen = projectHtml(
      model({ runs: [run({ state: "review", attention: { resume_at: iso(2), interacted: true, seen: true } })], plans: [] }),
      ui(),
    );
    expect(seen).toContain("sdot-seen");
  });

  it("marks the row whose surface is open", () => {
    expect(projectHtml(model(), ui({ activeRunId: "r1" }))).toContain('class="srow sentry active"');
  });

  it("routes each kind of row to its own surface", () => {
    const m = model({
      externalWorktrees: [worktree({ attention: { resume_at: iso(1), interacted: true, seen: true } })],
    });
    const html = projectHtml(m, ui());
    expect(html).toContain('data-run="r1"');
    expect(html).toContain('data-plan="pl1"');
    expect(html).toContain('data-wt="w1"');
  });

  it("shows the Worktrees row folded, and its contents when open", () => {
    const m = model({ externalWorktrees: [worktree({ worktree_id: "hidden", name: "hidden" })] });
    const folded = projectHtml(m, ui());
    expect(folded).toContain('data-wtline="p1"');
    expect(folded).not.toContain('data-wt="hidden"');
    expect(projectHtml(m, ui({ wtOpen: new Set(["p1"]) }))).toContain('data-wt="hidden"');
  });

  it("omits the Worktrees row when there is nothing behind it", () => {
    expect(projectHtml(model(), ui())).not.toContain("data-wtline");
  });

  it("says so when a project has nothing running", () => {
    expect(projectHtml(model({ runs: [], plans: [] }), ui())).toContain("Nothing running.");
  });

  it("escapes every string it renders", () => {
    const m = model({
      projects: [{ project_id: "p1", name: "<script>evil</script>" }],
      runs: [run({ goal: '<img src=x onerror="alert(1)">' })],
      plans: [],
    });
    const html = projectHtml(m, ui());
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;");
  });
});

describe("checkoutStatusHtml", () => {
  it("shows only the parts that have something to say", () => {
    expect(checkoutStatusHtml({ ahead: 0, behind: 0, insertions: 0, deletions: 0 })).toBe("");
    const busy = checkoutStatusHtml({ ahead: 1, behind: 2, insertions: 3, deletions: 4 });
    expect(busy).toContain("↑1");
    expect(busy).toContain("↓2");
    expect(busy).toContain("+3");
    expect(busy).toContain("-4");
  });
});

describe("sidebarHtml", () => {
  it("renders the header actions and an empty state", () => {
    const html = sidebarHtml([], ui());
    expect(html).toContain('id="side-add"');
    expect(html).toContain("No projects yet");
  });
});
