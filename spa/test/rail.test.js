import { describe, it, expect } from "vitest";
import { railEntries, railWorktrees, dotState, RAIL_MINIMUM } from "../src/core/rail.js";

const NOW = Date.parse("2026-07-28T09:00:00Z"); // Tuesday morning
const ago = (hours) => new Date(NOW - hours * 3600 * 1000).toISOString();

const run = (over = {}) => ({
  run_id: "r1",
  project_id: "p1",
  goal: "a run",
  branch: "build/a-run",
  state: "building",
  needs_attention: false,
  state_changed_at: ago(1),
  updated_at: ago(1),
  attention: { resume_at: ago(1), interacted: true, seen: true },
  stat: { insertions: 4, deletions: 1, ahead: 2, behind: 0 },
  ...over,
});

const plan = (over = {}) => ({
  plan_id: "pl1",
  project_id: "p1",
  goal: "an issue",
  state: "plan_review",
  needs_attention: true,
  state_changed_at: ago(1),
  updated_at: ago(1),
  attention: { resume_at: ago(1), interacted: true, seen: false },
  ...over,
});

const worktree = (over = {}) => ({
  worktree_id: "w1",
  project_id: "p1",
  name: "spike",
  branch: "build/spike",
  agent_working: false,
  dirty_files: 1,
  unpushed: 1,
  upstream: "origin/build/spike",
  behind_base: 2,
  base_branch: "main",
  diffstat: { files_changed: 1, insertions: 9, deletions: 2 },
  uncommitted: { files_changed: 1, insertions: 3, deletions: 1 },
  attention: { resume_at: ago(2), interacted: true, seen: false },
  ...over,
});

const ids = (entries) => entries.map((e) => e.id);

describe("what the rail shows", () => {
  it("keeps only this project's work", () => {
    const entries = railEntries({
      runs: [run(), run({ run_id: "other", project_id: "p2" })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["r1"]);
  });

  // Walking in: everything that was running when you left is still there, and
  // so is everything that finished overnight. (minimum 0 isolates this rule from
  // the backfill, which would otherwise top a short list up.)
  it("shows everything running and everything finished in the last day", () => {
    const entries = railEntries({
      runs: [
        run({ run_id: "still-going", state: "building", state_changed_at: ago(20) }),
        run({ run_id: "finished-overnight", state: "merged", state_changed_at: ago(14) }),
        run({ run_id: "finished-last-week", state: "merged", state_changed_at: ago(24 * 7) }),
      ],
      projectId: "p1",
      nowMs: NOW,
      minimum: 0,
    });
    expect(ids(entries).sort()).toEqual(["finished-overnight", "still-going"]);
  });

  // A review that has sat for a week is not "running" — treating it as such
  // would bury the work you did today under a backlog.
  it("does not treat a stale review as running", () => {
    const entries = railEntries({
      runs: [run({ run_id: "stale-review", state: "review", needs_attention: true, state_changed_at: ago(24 * 6) })],
      projectId: "p1",
      nowMs: NOW,
      minimum: 0,
    });
    expect(entries).toHaveLength(0);
  });

  it("has no ceiling — a busy day shows all of it", () => {
    const runs = Array.from({ length: 12 }, (_, i) =>
      run({ run_id: `r${i}`, state: "merged", state_changed_at: ago(2), attention: { resume_at: ago(i + 1), interacted: true, seen: true } }),
    );
    expect(railEntries({ runs, projectId: "p1", nowMs: NOW })).toHaveLength(12);
  });

  // Coming back after a weekend: nothing is running and nothing finished
  // yesterday, so the list fills with what is waiting on you — newest first —
  // and only then with whatever you had been working on.
  it("tops a thin list up with reviews first, then recent work", () => {
    const stale = (id, over) =>
      run({
        run_id: id,
        state: "merged",
        state_changed_at: ago(24 * 6),
        attention: { resume_at: ago(24 * 6), interacted: true, seen: true },
        ...over,
      });
    const runs = [
      stale("review-old", { state: "review", needs_attention: true, state_changed_at: ago(24 * 5) }),
      stale("review-new", { state: "review", needs_attention: true, state_changed_at: ago(24 * 3) }),
      stale("touched-recently", { attention: { resume_at: ago(24 * 2), interacted: true, seen: true } }),
      stale("touched-long-ago", { attention: { resume_at: ago(24 * 20), interacted: true, seen: true } }),
      stale("touched-longest-ago", { attention: { resume_at: ago(24 * 30), interacted: true, seen: true } }),
      stale("also-ancient", { attention: { resume_at: ago(24 * 40), interacted: true, seen: true } }),
      run({ run_id: "running-now", state: "building", state_changed_at: ago(1) }),
    ];
    const chosen = ids(railEntries({ runs, projectId: "p1", nowMs: NOW }));
    expect(chosen).toHaveLength(RAIL_MINIMUM);
    // The live one is always there; both reviews beat mere recency; the last
    // slot goes to the most recently touched of the rest.
    expect(chosen).toContain("running-now");
    expect(chosen).toContain("review-new");
    expect(chosen).toContain("review-old");
    expect(chosen).toContain("touched-recently");
    expect(chosen).not.toContain("also-ancient");
  });
});

describe("the order holds still while you work", () => {
  // The rule the whole rail rests on: the daemon keeps resume_at fixed while you
  // keep working on one thing, so its position cannot move under your hands.
  it("orders by resume point, oldest stretch first", () => {
    const entries = railEntries({
      runs: [
        run({ run_id: "picked-up-at-08", attention: { resume_at: "2026-07-28T08:00:00Z", interacted: true, seen: true } }),
        run({ run_id: "picked-up-at-07", attention: { resume_at: "2026-07-28T07:00:00Z", interacted: true, seen: true } }),
      ],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["picked-up-at-07", "picked-up-at-08"]);
  });

  it("puts a newly resumed entry at the bottom without disturbing the rest", () => {
    const base = [
      run({ run_id: "a", attention: { resume_at: ago(6), interacted: true, seen: true } }),
      run({ run_id: "b", attention: { resume_at: ago(4), interacted: true, seen: true } }),
      run({ run_id: "c", attention: { resume_at: ago(2), interacted: true, seen: true } }),
    ];
    expect(ids(railEntries({ runs: base, projectId: "p1", nowMs: NOW }))).toEqual(["a", "b", "c"]);
    // You pick "a" back up after a break: it moves to the bottom, b and c hold.
    const resumed = base.map((r) =>
      r.run_id === "a" ? { ...r, attention: { ...r.attention, resume_at: ago(0.1) } } : r,
    );
    expect(ids(railEntries({ runs: resumed, projectId: "p1", nowMs: NOW }))).toEqual(["b", "c", "a"]);
  });
});

describe("worktrees only enter once Build knows about them", () => {
  it("shows one Build cut for you and hides one you made by hand", () => {
    const entries = railEntries({
      worktrees: [
        worktree({ worktree_id: "built", attention: { resume_at: ago(1), interacted: true, seen: false } }),
        worktree({ worktree_id: "by-hand", attention: { resume_at: null, interacted: false, seen: false } }),
      ],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["built"]);
  });

  it("hands the rest to the Worktrees row, ordered by name", () => {
    const worktrees = [
      worktree({ worktree_id: "zeta", name: "zeta", attention: { interacted: false } }),
      worktree({ worktree_id: "alpha", name: "alpha", attention: { interacted: false } }),
      worktree({ worktree_id: "shown", name: "shown", attention: { resume_at: ago(1), interacted: true } }),
    ];
    const entries = railEntries({ worktrees, projectId: "p1", nowMs: NOW });
    const rows = railWorktrees({ worktrees, entries, projectId: "p1" });
    expect(rows.map((w) => w.name)).toEqual(["alpha", "zeta"]);
  });

  it("can order that row by recency instead", () => {
    const worktrees = [
      worktree({ worktree_id: "a", name: "alpha", attention: { resume_at: ago(9), interacted: false } }),
      worktree({ worktree_id: "z", name: "zeta", attention: { resume_at: ago(1), interacted: false } }),
    ];
    const rows = railWorktrees({ worktrees, entries: [], projectId: "p1", by: "recent" });
    expect(rows.map((w) => w.name)).toEqual(["zeta", "alpha"]);
  });
});

describe("the dot", () => {
  it("pulses only while an agent is working", () => {
    expect(dotState({ working: true, attention: { seen: false } })).toBe("working");
    expect(dotState({ working: true, attention: { seen: true } })).toBe("working");
  });

  it("is yellow when it has finished and you have not looked since", () => {
    expect(dotState({ working: false, attention: { seen: false } })).toBe("unseen");
  });

  it("settles to grey once seen", () => {
    expect(dotState({ working: false, attention: { seen: true } })).toBe("seen");
  });

  it("treats a worktree with a working agent as working, though it has no state", () => {
    const [entry] = railEntries({
      worktrees: [worktree({ agent_working: true, attention: { resume_at: ago(1), interacted: true, seen: true } })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(dotState(entry)).toBe("working");
  });
});

describe("the row's contents", () => {
  it("names a run by its goal and a worktree by its name, falling back to the branch", () => {
    const [runRow] = railEntries({ runs: [run({ goal: "fix the thing" })], projectId: "p1", nowMs: NOW });
    expect(runRow.name).toBe("fix the thing");
    const [wtRow] = railEntries({
      worktrees: [worktree({ name: "", branch: "build/no-name", attention: { resume_at: ago(1), interacted: true } })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(wtRow.name || wtRow.branch).toBe("build/no-name");
  });

  it("carries a run's unpushed/behind counts and the delta it is building", () => {
    const [entry] = railEntries({ runs: [run()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toMatchObject({ unpushed: 2, behind: 0, insertions: 4, deletions: 1 });
  });

  // Three separate questions, three separate answers: is any of this only
  // here (unpushed, vs the upstream), is it out of date (behind, vs the base
  // branch), and is any of it uncommitted (+/−, vs HEAD). The branch delta —
  // 9 insertions here — is the diff surface's business, not the row's.
  it("answers unpushed, out-of-date and uncommitted separately", () => {
    const [entry] = railEntries({ worktrees: [worktree()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toMatchObject({
      unpushed: 1,
      upstream: "origin/build/spike",
      behind: 2,
      base: "main",
      insertions: 3,
      deletions: 1,
    });
  });

  // An older bridge reports no uncommitted stat at all. Standing in the branch
  // delta there would answer a different question under the same label.
  it("shows no counts for a worktree the bridge sent none for", () => {
    const [entry] = railEntries({
      worktrees: [worktree({ uncommitted: undefined })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(entry.status).toMatchObject({ unpushed: 1, behind: 2, insertions: 0, deletions: 0 });
  });

  it("gives an issue no git status — it has no worktree to have one", () => {
    const [entry] = railEntries({ plans: [plan()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toBeNull();
  });
});
