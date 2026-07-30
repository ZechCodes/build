// What a project shows in the rail, and in what order.
//
// The rail answers three questions with one list. Walking in, you should see
// everything that was running when you left. Working, it should hold still.
// Coming back after a weekend, you should see what needs review and what you
// were last on — without a wall of work.
//
// So: show everything alive and everything finished in the last day; if that is
// thin, top it up first with work waiting on you, then with what you touched
// most recently. Order by RESUME POINT — when you picked a thing up, not when
// you last poked it — which the daemon maintains, so working on one entry all
// afternoon never moves it, while picking up something you had left drops it to
// the bottom.

const DAY_MS = 24 * 3600 * 1000;

/** The floor, not a ceiling: everything alive and everything from the last day
 *  is shown however many that is. Backfill only tops up a thin list. */
export const RAIL_MINIMUM = 5;

/** Run and plan states that mean an agent is mid-flight. */
const WORKING_RUN_STATES = new Set(["created", "building", "validating"]);
const WORKING_PLAN_STATES = new Set(["created", "drafting"]);
/** States that mean the entity is over and will not move again on its own. */
const TERMINAL_RUN_STATES = new Set(["merged", "abandoned", "archived"]);
const TERMINAL_PLAN_STATES = new Set(["abandoned"]);

const ms = (iso) => {
  const t = Date.parse(iso || "");
  return Number.isFinite(t) ? t : null;
};

/** The name a row carries: what the thing is called, falling back to the branch
 *  it lives on — a worktree with no goal behind it is still its branch. */
const nameOf = (entry) => entry.name || entry.branch || "";

/** Normalise a run into a rail entry. */
function runEntry(run) {
  const state = run.state || "";
  return {
    kind: "run",
    id: run.run_id,
    project_id: run.project_id,
    name: run.goal,
    branch: run.branch || "",
    state,
    working: WORKING_RUN_STATES.has(state),
    terminal: TERMINAL_RUN_STATES.has(state),
    needsYou: !!run.needs_attention,
    changedAt: ms(run.state_changed_at) ?? ms(run.updated_at),
    attention: run.attention || {},
    can_finish: !!run.can_finish,
    // Ahead and behind always use the same selected ref. +/- is only the dirty
    // working-tree delta; the task's full diff stays on its review surfaces.
    status: {
      ahead: run.stat?.ahead ?? null,
      behind: run.stat?.behind ?? null,
      comparisonRef: run.stat?.comparison_ref || null,
      insertions: run.stat?.uncommitted?.insertions ?? 0,
      deletions: run.stat?.uncommitted?.deletions ?? 0,
      changesLabel: "uncommitted",
    },
    route: { name: "task", projectId: run.project_id, id: run.run_id },
  };
}

/** Normalise a plan (an issue) into a rail entry. An issue has no worktree, so
 *  it has no git status to float — its dot and its name carry it. */
function planEntry(plan) {
  const state = plan.state || "";
  return {
    kind: "plan",
    id: plan.plan_id,
    project_id: plan.project_id,
    name: plan.goal,
    branch: "",
    state,
    working: WORKING_PLAN_STATES.has(state),
    terminal: TERMINAL_PLAN_STATES.has(state),
    needsYou: !!plan.needs_attention,
    changedAt: ms(plan.state_changed_at) ?? ms(plan.updated_at),
    attention: plan.attention || {},
    can_archive: !!plan.can_archive,
    archived_at: plan.archived_at || null,
    status: null,
    route: { name: "plan", projectId: plan.project_id, id: plan.issue_id || plan.plan_id, tab: "conversation" },
  };
}

/** Normalise a worktree into a rail entry. It has no lifecycle of its own: it
 *  is "working" while an agent paints in it, and otherwise sits waiting for you,
 *  which is why one Build cut for you arrives already pending. */
function worktreeEntry(worktree) {
  return {
    kind: "worktree",
    id: worktree.worktree_id,
    project_id: worktree.project_id,
    name: worktree.name || "",
    branch: worktree.branch || "",
    state: "",
    working: !!worktree.agent_working,
    agent_working: !!worktree.agent_working,
    terminal: false,
    needsYou: false,
    changedAt: ms(worktree.attention?.resume_at),
    attention: worktree.attention || {},
    can_finish: !!worktree.can_finish,
    dirty_files: worktree.dirty_files ?? 0,
    base_branch: worktree.base_branch || null,
    upstream: worktree.upstream || null,
    unpushed: worktree.unpushed ?? null,
    uncommitted: worktree.uncommitted || null,
    // Both commit counts use one selected ref; +/- is only uncommitted work.
    status: {
      ahead: worktree.ahead ?? null,
      behind: worktree.behind ?? null,
      comparisonRef: worktree.comparison_ref || null,
      insertions: worktree.uncommitted?.insertions ?? 0,
      deletions: worktree.uncommitted?.deletions ?? 0,
      changesLabel: "uncommitted",
    },
    route: { name: "worktree", projectId: worktree.project_id, worktreeId: worktree.worktree_id },
  };
}

/** The dot: what is true about this entry right now.
 *
 *  A pulse means an agent is working — never merely that a session is open — so
 *  it can be trusted to mean progress. Otherwise the entry is finished with your
 *  attention, and the only question is whether you have looked since it last
 *  moved. */
export function dotState(entry) {
  if (entry.working) return "working";
  return entry.attention?.seen ? "seen" : "unseen";
}

/** The sort key: when this stretch of work began. Entries the daemon has never
 *  seen touched fall back to whatever it reported, and anything unparseable
 *  sorts oldest — the top — rather than jumping to the bottom. */
const sortKey = (entry) => ms(entry.attention?.resume_at) ?? 0;

/**
 * The project's rail entries, in display order.
 *
 * `worktrees` are only eligible as entries once they have been interacted with
 * in Build — one Build cut for you arrives that way, one you made by hand does
 * not, and until then it belongs to the Worktrees row instead.
 */
export function railEntries({ runs = [], plans = [], worktrees = [], projectId, nowMs = Date.now(), minimum = RAIL_MINIMUM } = {}) {
  const mine = (list) => list.filter((x) => x.project_id === projectId);
  const candidates = [
    ...mine(runs).filter((run) => run.state !== "archived").map(runEntry),
    ...mine(plans).map(planEntry),
    ...mine(worktrees).filter((w) => w.attention?.interacted).map(worktreeEntry),
  ];

  const chosen = new Map();
  const take = (entry) => chosen.set(`${entry.kind}:${entry.id}`, entry);

  // Everything with an agent mid-flight, and everything that finished within the
  // day — merged, abandoned, or waiting on you. No ceiling: walking in after a
  // night, all of yesterday's work is the point.
  //
  // "Alive" is deliberately NOT "non-terminal": a review that has been sitting
  // for a week is not running, and treating it as such would both bury today's
  // work and leave the backfill below with nothing to do.
  for (const entry of candidates) {
    const finishedToday = entry.changedAt !== null && nowMs - entry.changedAt <= DAY_MS;
    if (entry.working || finishedToday) take(entry);
  }

  // Still thin? First the work that is waiting on you, newest first — coming
  // back after a weekend, that is what you came back for.
  const rest = () => candidates.filter((entry) => !chosen.has(`${entry.kind}:${entry.id}`));
  if (chosen.size < minimum) {
    rest()
      .filter((entry) => entry.needsYou)
      .sort((a, b) => (b.changedAt ?? 0) - (a.changedAt ?? 0))
      .slice(0, minimum - chosen.size)
      .forEach(take);
  }
  // Then simply what you were last working on.
  if (chosen.size < minimum) {
    rest()
      .sort((a, b) => sortKey(b) - sortKey(a))
      .slice(0, minimum - chosen.size)
      .forEach(take);
  }

  // Oldest stretch of work first, so a newly resumed entry lands at the bottom
  // and nothing above it moves. Ties break by name for a stable paint.
  return [...chosen.values()].sort(
    (a, b) => sortKey(a) - sortKey(b) || nameOf(a).localeCompare(nameOf(b)),
  );
}

/**
 * The worktrees the `Worktrees ›` row holds: everything not already an entry
 * above it. Ordered by name — this is a place to look something up, not a
 * feed — or by recency when the caller asks.
 */
export function railWorktrees({ worktrees = [], entries = [], projectId, by = "name" } = {}) {
  const shown = new Set(entries.filter((e) => e.kind === "worktree").map((e) => e.id));
  const rows = worktrees
    .filter((w) => w.project_id === projectId && !shown.has(w.worktree_id))
    .map(worktreeEntry);
  return by === "recent"
    ? rows.sort((a, b) => (ms(b.attention?.resume_at) ?? 0) - (ms(a.attention?.resume_at) ?? 0))
    : rows.sort((a, b) => nameOf(a).localeCompare(nameOf(b)));
}
