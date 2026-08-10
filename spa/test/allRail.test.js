// The flat rail: one list of everything Build knows about, ungrouped. WHICH
// entries appear and in what order is rail.js's job (and rail.test.js's); this
// covers the model that feeds every project's work into one selection, and the
// markup that labels a row no longer sitting under a project name.

import { describe, it, expect } from "vitest";
import { buildAllRailModel } from "../src/core/allRail.js";
import { allRailHtml } from "../src/core/sidebar.js";

const NOW = Date.parse("2026-07-28T12:00:00Z");
const iso = (hoursAgo) => new Date(NOW - hoursAgo * 3600 * 1000).toISOString();

const projects = [
  { project_id: "p1", name: "relaydb" },
  { project_id: "p2", name: "dotfiles" },
];

const run = (over = {}) => ({
  run_id: "r1",
  project_id: "p1",
  goal: "Fix the thing",
  branch: "build/fix-the-thing",
  state: "review",
  needs_attention: true,
  can_finish: false,
  stat: {
    files_changed: 2,
    insertions: 420,
    deletions: 260,
    comparison_ref: "origin/build/fix-the-thing",
    ahead: 3,
    behind: 1,
    uncommitted: { files_changed: 2, insertions: 42, deletions: 26 },
  },
  state_changed_at: iso(1),
  updated_at: iso(1),
  attention: { resume_at: iso(2), interacted: true, seen: false },
  ...over,
});

const plan = (over = {}) => ({
  plan_id: "pl1",
  project_id: "p1",
  goal: "Design the thing",
  state: "plan_review",
  needs_attention: true,
  state_changed_at: iso(1),
  updated_at: iso(1),
  attention: { resume_at: iso(3), interacted: true, seen: true },
  can_archive: false,
  ...over,
});

/** A worktree with nothing to say: the floor has no claim on it, so it is what a
 *  test uses to talk about the ordinary rules (see rail.test.js's quietWorktree). */
const worktree = (over = {}) => ({
  worktree_id: "w1",
  project_id: "p1",
  name: "spike",
  branch: "build/spike",
  agent_working: false,
  dirty_files: 0,
  ahead: 0,
  behind: 0,
  comparison_ref: "main",
  upstream: null,
  base_branch: "main",
  diffstat: { files_changed: 0, insertions: 0, deletions: 0 },
  uncommitted: { files_changed: 0, insertions: 0, deletions: 0 },
  attention: { resume_at: iso(9), interacted: false, seen: true },
  can_finish: false,
  ...over,
});

const primaryChange = (over = {}) => ({
  project_id: "p1",
  branch: "main",
  path: "/repos/relaydb",
  upstream: "origin/main",
  comparison_ref: "origin/main",
  ahead: 0,
  behind: 0,
  files_changed: 0,
  insertions: 0,
  deletions: 0,
  run_id: null,
  ...over,
});

const model = (over = {}) =>
  buildAllRailModel({
    projects,
    runs: [run()],
    plans: [plan()],
    externalWorktrees: [],
    primaryChanges: [],
    readIds: new Set(),
    nowMs: NOW,
    ...over,
  });

const ui = (over = {}) => ({
  closed: new Set(),
  wtOpen: new Set(),
  mode: "all",
  activeRunId: null,
  activePlanId: null,
  activeWorktreeId: null,
  activeProjectId: null,
  activeMainProjectId: null,
  ...over,
});

const ids = (entries) => entries.map((e) => e.id);

describe("buildAllRailModel", () => {
  it("puts every project's work in one list, ordered by resume point", () => {
    const m = model({
      runs: [
        run({ run_id: "here-late", project_id: "p1", attention: { resume_at: iso(1), interacted: true, seen: true } }),
        run({ run_id: "there-middle", project_id: "p2", attention: { resume_at: iso(5), interacted: true, seen: true } }),
        run({ run_id: "here-early", project_id: "p1", attention: { resume_at: iso(9), interacted: true, seen: true } }),
      ],
      plans: [],
    });
    // Interleaved across projects — an order grouping could not produce.
    expect(ids(m.entries)).toEqual(["here-early", "there-middle", "here-late"]);
  });

  it("says which project each row belongs to", () => {
    const m = model({
      runs: [run({ run_id: "mine" }), run({ run_id: "theirs", project_id: "p2" })],
      plans: [],
    });
    expect(m.entries.map((e) => e.project_name).sort()).toEqual(["dotfiles", "relaydb"]);
  });

  it("gives a project with a dirty checkout a main row of its own", () => {
    const m = model({
      runs: [],
      plans: [],
      primaryChanges: [primaryChange({ files_changed: 1, insertions: 12, deletions: 4 })],
    });
    expect(m.entries).toHaveLength(1);
    expect(m.entries[0]).toMatchObject({
      kind: "main",
      id: "p1",
      project_name: "relaydb",
      branch: "main",
      status: { insertions: 12, deletions: 4 },
    });
  });

  it("lights a main row with the dot of the run that adopted the checkout", () => {
    const m = model({
      runs: [run({ run_id: "rp", primary: true, state: "building", goal: "" })],
      plans: [],
      primaryChanges: [primaryChange({ run_id: "rp" })],
    });
    expect(ids(m.entries)).toEqual(["p1"]);
    expect(m.entries[0]).toMatchObject({ kind: "main", working: true, run_id: "rp" });
  });

  // The run that owns a checkout IS the main row. Listing it twice would say an
  // agent is in two places at once.
  it("does not list the primary run beside its own main row", () => {
    const m = model({
      runs: [run(), run({ run_id: "rp", primary: true, goal: "", branch: "main" })],
      plans: [],
      primaryChanges: [primaryChange({ run_id: "rp" })],
    });
    expect(ids(m.entries).sort()).toEqual(["p1", "r1"]);
  });

  it("finds the owning run by the id the checkout names, whoever else is primary", () => {
    const m = model({
      runs: [run({ run_id: "named", primary: false, state: "building" })],
      plans: [],
      primaryChanges: [primaryChange({ run_id: "named" })],
    });
    expect(ids(m.entries)).toEqual(["p1"]);
    expect(m.entries[0]).toMatchObject({ run_id: "named", working: true });
  });

  it("shows a busy or dirty main checkout however much newer work is above it", () => {
    const busy = Array.from({ length: 8 }, (_, i) =>
      run({
        run_id: `today-${i}`,
        state: "merged",
        needs_attention: false,
        state_changed_at: iso(1),
        attention: { resume_at: iso(i + 1), interacted: true, seen: true },
      }),
    );
    const m = model({
      runs: [...busy, run({ run_id: "rp2", project_id: "p2", primary: true, state: "building", goal: "" })],
      plans: [],
      primaryChanges: [
        primaryChange({ files_changed: 1, insertions: 3, deletions: 0 }),
        primaryChange({ project_id: "p2", run_id: "rp2" }),
      ],
    });
    expect(ids(m.entries)).toContain("p1"); // dirty, nobody adopted it
    expect(ids(m.entries)).toContain("p2"); // an agent is painting in it
  });

  it("leaves a clean, quiet checkout to the ordinary rules", () => {
    const m = model({ runs: [], plans: [], primaryChanges: [primaryChange()] });
    // Nothing else is competing, so the backfill still offers it a slot.
    expect(ids(m.entries)).toEqual(["p1"]);
    const busy = model({
      runs: Array.from({ length: 6 }, (_, i) =>
        run({ run_id: `today-${i}`, attention: { resume_at: iso(i + 1), interacted: true, seen: true } }),
      ),
      plans: [],
      primaryChanges: [primaryChange({ project_id: "p2" })],
    });
    expect(ids(busy.entries)).not.toContain("p2");
  });

  it("counts unread work waiting on you across every project", () => {
    const m = model({
      runs: [run(), run({ run_id: "theirs", project_id: "p2" })],
      plans: [plan({ needs_attention: true })],
      readIds: new Set(["r1"]),
    });
    expect(m.unread).toBe(2); // the p2 run and the issue
  });

  it("omits entries whose confirmed Done work is still running in the background", () => {
    const m = model({
      externalWorktrees: [worktree({ attention: { resume_at: iso(1), interacted: true, seen: true } })],
      pendingDone: new Set(["run:r1", "plan:pl1", "worktree:w1"]),
    });
    expect(m.entries).toEqual([]);
    expect(m.worktrees).toEqual([]);
    expect(m.unread).toBe(0);
  });

  it("hands every project's leftover worktrees to one list, ordered by name", () => {
    const m = model({
      runs: [],
      plans: [],
      externalWorktrees: [
        worktree({ worktree_id: "z", name: "zeta", project_id: "p2" }),
        worktree({ worktree_id: "a", name: "alpha", project_id: "p1" }),
        worktree({
          worktree_id: "shown",
          name: "shown",
          attention: { resume_at: iso(1), interacted: true, seen: true },
        }),
      ],
    });
    expect(ids(m.entries)).toContain("shown");
    expect(m.worktrees.map((w) => w.name)).toEqual(["alpha", "zeta"]);
    expect(m.worktrees.map((w) => w.project_name)).toEqual(["relaydb", "dotfiles"]);
  });
});

describe("the flat rail's markup", () => {
  it("renders the switch with All on, and keeps the add button", () => {
    const html = allRailHtml(model(), ui());
    expect(html).toContain('data-railmode="all"');
    expect(html).toContain('data-railmode="projects"');
    expect(html).toContain('id="side-add"');
    const all = html.match(/<button[^>]*data-railmode="all"[^>]*>/)[0];
    expect(all).toContain("active");
    expect(all).toContain('aria-pressed="true"');
    const projectsButton = html.match(/<button[^>]*data-railmode="projects"[^>]*>/)[0];
    expect(projectsButton).toContain('aria-pressed="false"');
  });

  it("routes each kind of row to its own surface", () => {
    const m = model({
      externalWorktrees: [worktree({ attention: { resume_at: iso(1), interacted: true, seen: true } })],
      primaryChanges: [primaryChange({ files_changed: 1, insertions: 2, deletions: 0 })],
    });
    const html = allRailHtml(m, ui());
    expect(html).toContain('data-run="r1"');
    expect(html).toContain('data-plan="pl1"');
    expect(html).toContain('data-wt="w1"');
    expect(html).toContain('data-main="p1"');
    expect(html).toContain('data-project="p1"');
  });

  it("says which project a row belongs to, since nothing groups it now", () => {
    const html = allRailHtml(model({ runs: [run({ project_id: "p2" })], plans: [] }), ui());
    expect(html).toContain('class="sproj-tag mono">dotfiles<');
  });

  it("shows a main row as its branch and status, with nothing to finish", () => {
    const m = model({
      runs: [],
      plans: [],
      primaryChanges: [primaryChange({ ahead: 2, behind: 1, files_changed: 1, insertions: 5, deletions: 3 })],
    });
    const html = allRailHtml(m, ui());
    expect(html).toContain("↑2");
    expect(html).toContain("↓1");
    expect(html).toContain("+5");
    expect(html).toContain("-3");
    expect(html).toContain("main");
    expect(html).not.toContain("data-done-");
  });

  it("marks the row whose surface is open, main rows included", () => {
    expect(allRailHtml(model(), ui({ activeRunId: "r1" }))).toContain("srow sentry active");
    const withMain = model({ runs: [], plans: [], primaryChanges: [primaryChange({ files_changed: 1, insertions: 1 })] });
    expect(allRailHtml(withMain, ui({ activeMainProjectId: "p1" }))).toContain("active");
  });

  it("folds every leftover worktree into one row, and opens it under __all", () => {
    const m = model({ externalWorktrees: [worktree({ worktree_id: "hidden", name: "hidden" })] });
    const folded = allRailHtml(m, ui());
    expect(folded).toContain('data-wtline="__all"');
    expect(folded).not.toContain('data-wt="hidden"');
    const open = allRailHtml(m, ui({ wtOpen: new Set(["__all"]) }));
    expect(open).toContain('data-wt="hidden"');
    expect(open).toContain("relaydb");
  });

  it("says so when nothing is running anywhere", () => {
    expect(allRailHtml(model({ runs: [], plans: [] }), ui())).toContain("Nothing running.");
  });

  it("escapes every string it renders", () => {
    const m = buildAllRailModel({
      projects: [{ project_id: "p1", name: "<script>evil</script>" }],
      runs: [run({ goal: '<img src=x onerror="alert(1)">' })],
      plans: [],
      externalWorktrees: [],
      primaryChanges: [],
      readIds: new Set(),
      nowMs: NOW,
    });
    const html = allRailHtml(m, ui());
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;");
  });
});
