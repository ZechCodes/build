import { describe, it, expect } from "vitest";
import {
  railEntries,
  railWorktrees,
  dotState,
  mainEntry,
  hasUnreviewedChanges,
  mustShow,
  RAIL_MINIMUM,
} from "../src/core/rail.js";

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
  stat: {
    insertions: 40,
    deletions: 10,
    comparison_ref: "main",
    ahead: 2,
    behind: 0,
    uncommitted: { files_changed: 1, insertions: 4, deletions: 1 },
  },
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
  can_archive: false,
  archived_at: null,
  ...over,
});

const worktree = (over = {}) => ({
  worktree_id: "w1",
  project_id: "p1",
  name: "spike",
  branch: "build/spike",
  agent_working: false,
  dirty_files: 1,
  ahead: 1,
  behind: 2,
  comparison_ref: "origin/build/spike",
  unpushed: 1,
  upstream: "origin/build/spike",
  base_branch: "main",
  diffstat: { files_changed: 1, insertions: 9, deletions: 2 },
  uncommitted: { files_changed: 1, insertions: 3, deletions: 1 },
  attention: { resume_at: ago(2), interacted: true, seen: false },
  can_finish: false,
  ...over,
});

const project = (over = {}) => ({ project_id: "p1", name: "relaydb", ...over });

/** One project's `primary_changes` record: the branch its checkout has out, how
 *  that branch stands against its comparison ref, and what is uncommitted in it.
 *  It carries no attention of its own — the run that adopted the checkout, if
 *  any, is where liveness and read-state live. */
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

/** A worktree with nothing in it and nothing to say: no agent, no uncommitted
 *  work, nothing ahead, and already looked at. The floor has no claim on it, so
 *  it is what a test uses to talk about the ordinary rules. */
const quietWorktree = (over = {}) => ({
  ...worktree({
    agent_working: false,
    dirty_files: 0,
    ahead: 0,
    behind: 0,
    unpushed: 0,
    diffstat: { files_changed: 0, insertions: 0, deletions: 0 },
    uncommitted: { files_changed: 0, insertions: 0, deletions: 0 },
    attention: { resume_at: ago(24 * 9), interacted: false, seen: true },
  }),
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

// The floor under everything above: age and the backfill decide how much of the
// quiet work to show, but they never get a vote on work that is live, unread, or
// carrying a diff nobody has reviewed.
describe("what the rail can never drop", () => {
  it("shows a worktree with an agent in it, though Build never touched it", () => {
    const entries = railEntries({
      worktrees: [
        quietWorktree({
          worktree_id: "by-hand",
          agent_working: true,
          attention: { resume_at: ago(24 * 30), interacted: false, seen: true },
        }),
      ],
      projectId: "p1",
      nowMs: NOW,
      minimum: 0,
    });
    expect(ids(entries)).toEqual(["by-hand"]);
  });

  it("shows a worktree holding uncommitted work nobody has looked at, however old", () => {
    const entries = railEntries({
      worktrees: [
        quietWorktree({
          worktree_id: "dirty",
          uncommitted: { files_changed: 1, insertions: 3, deletions: 1 },
          attention: { resume_at: ago(24 * 30), interacted: false, seen: false },
        }),
      ],
      projectId: "p1",
      nowMs: NOW,
      minimum: 0,
    });
    expect(ids(entries)).toEqual(["dirty"]);
  });

  it("still leaves a stale, clean, already-seen worktree to the Worktrees row", () => {
    const worktrees = [quietWorktree({ worktree_id: "settled", name: "settled" })];
    const entries = railEntries({ worktrees, projectId: "p1", nowMs: NOW, minimum: 0 });
    expect(entries).toEqual([]);
    expect(railWorktrees({ worktrees, entries, projectId: "p1" }).map((w) => w.id)).toEqual(["settled"]);
  });

  // The backfill cannot be what saves it: five entries already fill the list.
  it("shows a week-old review waiting on you that a busy day would have crowded out", () => {
    const busy = Array.from({ length: RAIL_MINIMUM }, (_, i) =>
      run({
        run_id: `merged-today-${i}`,
        state: "merged",
        state_changed_at: ago(2),
        attention: { resume_at: ago(i + 1), interacted: true, seen: true },
      }),
    );
    const entries = railEntries({
      runs: [
        ...busy,
        run({
          run_id: "unread-review",
          state: "review",
          needs_attention: true,
          state_changed_at: ago(24 * 7),
          attention: { resume_at: ago(24 * 7), interacted: true, seen: false },
        }),
      ],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(entries).toHaveLength(RAIL_MINIMUM + 1);
    expect(ids(entries)).toContain("unread-review");
  });

  it("does not force-show a stale entry whose diff you have already seen", () => {
    const entries = railEntries({
      runs: [
        run({
          run_id: "seen-review",
          state: "review",
          needs_attention: true,
          state_changed_at: ago(24 * 6),
          attention: { resume_at: ago(24 * 6), interacted: true, seen: true },
        }),
      ],
      projectId: "p1",
      nowMs: NOW,
      minimum: 0,
    });
    expect(entries).toEqual([]);
  });
});

describe("whether an entry holds work you have not reviewed", () => {
  const entry = (over = {}) => ({
    working: false,
    needsYou: false,
    attention: { seen: true },
    status: { ahead: 0, behind: 0, insertions: 0, deletions: 0 },
    ...over,
  });

  it("counts uncommitted work you have not looked at", () => {
    expect(hasUnreviewedChanges(entry({ attention: { seen: false }, status: { insertions: 3, deletions: 0 } }))).toBe(true);
  });

  it("counts commits ahead of the comparison ref you have not looked at", () => {
    expect(hasUnreviewedChanges(entry({ attention: { seen: false }, status: { ahead: 2 } }))).toBe(true);
  });

  it("counts nothing once you have seen it, however big the diff", () => {
    expect(hasUnreviewedChanges(entry({ status: { ahead: 9, insertions: 400, deletions: 300 } }))).toBe(false);
  });

  it("counts nothing when there is no diff to review", () => {
    expect(hasUnreviewedChanges(entry({ attention: { seen: false } }))).toBe(false);
    expect(hasUnreviewedChanges(entry({ attention: { seen: false }, status: null }))).toBe(false);
  });

  it("keeps a live, an unread, or an unreviewed entry — and only those", () => {
    expect(mustShow(entry({ working: true }))).toBe(true);
    expect(mustShow(entry({ needsYou: true, attention: { seen: false } }))).toBe(true);
    expect(mustShow(entry({ attention: { seen: false }, status: { insertions: 1 } }))).toBe(true);
    expect(mustShow(entry({ needsYou: true }))).toBe(false);
    expect(mustShow(entry())).toBe(false);
  });
});

// A project's own checkout is work like any other: the main branch is where an
// adopted agent paints, and where uncommitted changes sit until someone looks.
describe("a project's main checkout as a row", () => {
  it("is named by its branch and carries the project it belongs to", () => {
    const entry = mainEntry({ project: project(), primaryChange: primaryChange() });
    expect(entry).toMatchObject({
      kind: "main",
      id: "p1",
      project_id: "p1",
      project_name: "relaydb",
      name: "main",
      branch: "main",
      path: "/repos/relaydb",
    });
  });

  it("shows how the checkout stands and what is uncommitted in it", () => {
    const entry = mainEntry({
      project: project(),
      primaryChange: primaryChange({ ahead: 2, behind: 1, insertions: 12, deletions: 4 }),
    });
    expect(entry.status).toEqual({
      ahead: 2,
      behind: 1,
      comparisonRef: "origin/main",
      insertions: 12,
      deletions: 4,
      changesLabel: "uncommitted",
    });
  });

  it("falls back to the upstream when no comparison ref was reported", () => {
    const entry = mainEntry({
      project: project(),
      primaryChange: primaryChange({ comparison_ref: null }),
    });
    expect(entry.status.comparisonRef).toBe("origin/main");
  });

  it("opens the project's Changes tab", () => {
    expect(mainEntry({ project: project(), primaryChange: primaryChange() }).route).toEqual({
      name: "project",
      projectId: "p1",
      tab: "changes",
    });
  });

  // Nothing has adopted the checkout, so there is no lifecycle to read: it is
  // quiet until its working tree says otherwise.
  it("is quiet with no run owning it, and shown the moment it is dirty", () => {
    const clean = mainEntry({ project: project(), primaryChange: primaryChange() });
    expect(clean).toMatchObject({ working: false, needsYou: false, changedAt: null, attention: {}, run_id: null });
    expect(mustShow(clean)).toBe(false);
    const dirty = mainEntry({
      project: project(),
      primaryChange: primaryChange({ files_changed: 1, insertions: 3, deletions: 1 }),
    });
    expect(mustShow(dirty)).toBe(true);
  });

  it("takes its dot and its clock from the run that adopted it", () => {
    const owner = run({ run_id: "rp", primary: true, state: "building", state_changed_at: ago(3) });
    const entry = mainEntry({
      project: project(),
      primaryChange: primaryChange({ run_id: "rp" }),
      run: owner,
    });
    expect(entry).toMatchObject({ working: true, run_id: "rp", changedAt: Date.parse(ago(3)) });
    expect(dotState(entry)).toBe("working");
    expect(mustShow(entry)).toBe(true);
  });

  it("is unread while its run waits on you and you have not looked", () => {
    const entry = mainEntry({
      project: project(),
      primaryChange: primaryChange(),
      run: run({
        run_id: "rp",
        state: "review",
        needs_attention: true,
        attention: { resume_at: ago(30), interacted: true, seen: false },
      }),
    });
    expect(entry.needsYou).toBe(true);
    expect(dotState(entry)).toBe("unseen");
    expect(mustShow(entry)).toBe(true);
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

describe("worktrees enter once Build knows them, or once they hold work", () => {
  it("shows one Build cut for you and hides a quiet one you made by hand", () => {
    const entries = railEntries({
      worktrees: [
        worktree({ worktree_id: "built", attention: { resume_at: ago(1), interacted: true, seen: false } }),
        quietWorktree({ worktree_id: "by-hand", attention: { resume_at: null, interacted: false, seen: true } }),
      ],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["built"]);
  });

  it("hands the rest to the Worktrees row, ordered by name", () => {
    const worktrees = [
      quietWorktree({ worktree_id: "zeta", name: "zeta" }),
      quietWorktree({ worktree_id: "alpha", name: "alpha" }),
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

// Asked for one project, the rail answers for that project. Asked for none, it
// answers for everything Build knows about — one selection, one backfill, one
// order, with nothing grouped.
describe("asked about every project at once", () => {
  it("takes work from every project into one list, ordered by resume point", () => {
    const entries = railEntries({
      runs: [
        run({ run_id: "here-late", project_id: "p1", attention: { resume_at: ago(2), interacted: true, seen: true } }),
        run({ run_id: "there-early", project_id: "p2", attention: { resume_at: ago(6), interacted: true, seen: true } }),
        run({ run_id: "here-early", project_id: "p1", attention: { resume_at: ago(9), interacted: true, seen: true } }),
      ],
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["here-early", "there-early", "here-late"]);
  });

  it("answers exactly as before when asked about one project", () => {
    const runs = [run(), run({ run_id: "other", project_id: "p2" })];
    expect(ids(railEntries({ runs, projectId: "p1", nowMs: NOW }))).toEqual(["r1"]);
  });

  it("backfills a thin list once across all projects, not once per project", () => {
    const stale = (id, projectId, hours) =>
      run({
        run_id: id,
        project_id: projectId,
        state: "merged",
        state_changed_at: ago(24 * 6),
        attention: { resume_at: ago(hours), interacted: true, seen: true },
      });
    const runs = [
      stale("p1-a", "p1", 24 * 2),
      stale("p1-b", "p1", 24 * 3),
      stale("p1-c", "p1", 24 * 4),
      stale("p2-a", "p2", 24 * 5),
      stale("p2-b", "p2", 24 * 6),
      stale("p2-c", "p2", 24 * 7),
    ];
    expect(railEntries({ runs, nowMs: NOW })).toHaveLength(RAIL_MINIMUM);
  });

  it("sorts main rows in with everything else instead of pinning them anywhere", () => {
    const mains = [
      mainEntry({
        project: project({ project_id: "p2", name: "dotfiles" }),
        primaryChange: primaryChange({ project_id: "p2" }),
        run: run({ run_id: "rp2", project_id: "p2", attention: { resume_at: ago(5), interacted: true, seen: true } }),
      }),
    ];
    const entries = railEntries({
      runs: [
        run({ run_id: "older", attention: { resume_at: ago(8), interacted: true, seen: true } }),
        run({ run_id: "newer", attention: { resume_at: ago(1), interacted: true, seen: true } }),
      ],
      mains,
      nowMs: NOW,
    });
    expect(ids(entries)).toEqual(["older", "p2", "newer"]);
  });

  it("never drops a main checkout holding uncommitted work, and lets a clean quiet one go", () => {
    const dirty = mainEntry({
      project: project(),
      primaryChange: primaryChange({ files_changed: 1, insertions: 5, deletions: 2 }),
    });
    const clean = mainEntry({
      project: project({ project_id: "p2", name: "dotfiles" }),
      primaryChange: primaryChange({ project_id: "p2" }),
    });
    const entries = railEntries({ mains: [dirty, clean], nowMs: NOW, minimum: 0 });
    expect(ids(entries)).toEqual(["p1"]);
  });

  it("keeps main rows out of a project's own rail when they belong elsewhere", () => {
    const mains = [
      mainEntry({ project: project(), primaryChange: primaryChange({ files_changed: 1, insertions: 5 }) }),
      mainEntry({
        project: project({ project_id: "p2", name: "dotfiles" }),
        primaryChange: primaryChange({ project_id: "p2", files_changed: 1, insertions: 5 }),
      }),
    ];
    expect(ids(railEntries({ mains, projectId: "p1", nowMs: NOW, minimum: 0 }))).toEqual(["p1"]);
  });

  it("hands the leftover worktrees of every project to one row, ordered by name", () => {
    const worktrees = [
      quietWorktree({ worktree_id: "z", name: "zeta", project_id: "p2" }),
      quietWorktree({ worktree_id: "a", name: "alpha", project_id: "p1" }),
    ];
    const rows = railWorktrees({ worktrees, entries: railEntries({ worktrees, nowMs: NOW, minimum: 0 }) });
    expect(rows.map((w) => w.name)).toEqual(["alpha", "zeta"]);
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
  it("never returns archived runs to the active rail", () => {
    expect(
      railEntries({
        runs: [run({ state: "archived", state_changed_at: ago(1) })],
        projectId: "p1",
        nowMs: NOW,
      }),
    ).toEqual([]);
  });

  it("retains plan archive eligibility and archive state", () => {
    const [entry] = railEntries({
      plans: [plan({ can_archive: true, archived_at: "2026-07-28T08:00:00Z" })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(entry).toMatchObject({ can_archive: true, archived_at: "2026-07-28T08:00:00Z" });
  });

  it("retains every external-worktree field needed to finish it", () => {
    const [entry] = railEntries({
      worktrees: [worktree({ can_finish: true })],
      projectId: "p1",
      nowMs: NOW,
    });
    expect(entry).toMatchObject({
      can_finish: true,
      agent_working: false,
      dirty_files: 1,
      branch: "build/spike",
      base_branch: "main",
      upstream: "origin/build/spike",
      unpushed: 1,
      uncommitted: { files_changed: 1, insertions: 3, deletions: 1 },
    });
  });

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

  it("compares a run with one ref and shows only its uncommitted delta", () => {
    const [entry] = railEntries({ runs: [run()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toMatchObject({
      ahead: 2,
      behind: 0,
      comparisonRef: "main",
      insertions: 4,
      deletions: 1,
    });
  });

  // Ahead and behind share one comparison ref. The branch delta — 9 insertions
  // here — is the diff surface's business, while +/- is only uncommitted work.
  it("answers ahead, behind and uncommitted separately", () => {
    const [entry] = railEntries({ worktrees: [worktree()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toMatchObject({
      ahead: 1,
      behind: 2,
      comparisonRef: "origin/build/spike",
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
    expect(entry.status).toMatchObject({ ahead: 1, behind: 2, insertions: 0, deletions: 0 });
  });

  it("gives an issue no git status — it has no worktree to have one", () => {
    const [entry] = railEntries({ plans: [plan()], projectId: "p1", nowMs: NOW });
    expect(entry.status).toBeNull();
  });
});
