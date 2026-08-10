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
//
// Under all of that sits a floor age cannot reach: anywhere an agent is running,
// anywhere something unread is waiting on you, and anywhere there are changes
// nobody has reviewed is always a row, however long ago it happened.

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

/** Normalise a run into a rail entry. Exported for the one run that never
 *  becomes a rail row: the primary checkout's owner, whose entry rides the
 *  project's checkout line instead. */
export function runEntry(run) {
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

/** Normalise a project's primary checkout into a rail entry: the main branch as
 *  a row of its own. The checkout has no lifecycle — an agent working against it
 *  adopts it as a run first — so liveness and read-state come from the run that
 *  owns it, and a checkout nobody has adopted is quiet until its working tree
 *  says otherwise. */
export function mainEntry({ project, primaryChange, run = null }) {
  const state = run?.state || "";
  return {
    kind: "main",
    // The row IS the project: one checkout, one project, one identity.
    id: project.project_id,
    project_id: project.project_id,
    project_name: project.name || "",
    // A main row is named by the branch it has out; there is no goal behind it.
    name: primaryChange.branch || "",
    branch: primaryChange.branch || "",
    path: primaryChange.path || null,
    state: "",
    working: WORKING_RUN_STATES.has(state),
    terminal: false,
    needsYou: !!run?.needs_attention,
    changedAt: run ? (ms(run.state_changed_at) ?? ms(run.updated_at)) : null,
    attention: run?.attention || {},
    run_id: run?.run_id || primaryChange.run_id || null,
    // The checkout's own +/- is what sits in it uncommitted; ahead and behind
    // use whichever ref the bridge compared it against.
    status: {
      ahead: primaryChange.ahead ?? null,
      behind: primaryChange.behind ?? null,
      comparisonRef: primaryChange.comparison_ref || primaryChange.upstream || null,
      insertions: primaryChange.insertions ?? 0,
      deletions: primaryChange.deletions ?? 0,
      changesLabel: "uncommitted",
    },
    route: { name: "project", projectId: project.project_id, tab: "changes" },
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

/** Does this entry hold a diff nobody has signed off on? There is no per-diff
 *  "reviewed" mark anywhere in Build, so the honest reading is: there is work
 *  here — uncommitted, or committed ahead of the ref it is compared against —
 *  and the entry has not been looked at since it last moved. */
export function hasUnreviewedChanges(entry) {
  if (dotState(entry) === "seen") return false;
  const status = entry.status;
  if (!status) return false;
  return (status.insertions ?? 0) + (status.deletions ?? 0) > 0 || (status.ahead ?? 0) > 0;
}

/** The floor: is this entry one the rail is not allowed to drop, whatever its
 *  age? An agent is running in it, it is waiting on you and you have not looked,
 *  or it holds changes nobody has reviewed. */
export function mustShow(entry) {
  if (entry.working) return true;
  if (entry.needsYou && dotState(entry) !== "seen") return true;
  return hasUnreviewedChanges(entry);
}

/** The sort key: when this stretch of work began. Entries the daemon has never
 *  seen touched fall back to whatever it reported, and anything unparseable
 *  sorts oldest — the top — rather than jumping to the bottom. */
const sortKey = (entry) => ms(entry.attention?.resume_at) ?? 0;

/**
 * The rail entries, in display order — for one project, or for every project at
 * once when no `projectId` is given. Either way it is one selection, one
 * backfill and one order; nothing is grouped.
 *
 * `mains` are already-built `mainEntry` rows, which take part on exactly the
 * same terms as everything else.
 *
 * `worktrees` are only eligible as entries once they have been interacted with
 * in Build — one Build cut for you arrives that way, one you made by hand does
 * not, and until then it belongs to the Worktrees row instead. The floor is the
 * exception: a worktree with an agent in it, or holding work nobody has
 * reviewed, is exactly what the rail exists to surface, however it was made.
 */
export function railEntries({ runs = [], plans = [], worktrees = [], mains = [], projectId = null, nowMs = Date.now(), minimum = RAIL_MINIMUM } = {}) {
  const mine = (list) => (projectId === null ? list : list.filter((x) => x.project_id === projectId));
  const candidates = [
    ...mine(runs).filter((run) => run.state !== "archived").map(runEntry),
    ...mine(plans).map(planEntry),
    ...mine(worktrees)
      .map(worktreeEntry)
      .filter((entry) => entry.attention?.interacted || mustShow(entry)),
    ...mine(mains),
  ];

  const chosen = new Map();
  const take = (entry) => chosen.set(`${entry.kind}:${entry.id}`, entry);

  // Everything with an agent mid-flight, and everything that finished within the
  // day — merged, abandoned, or waiting on you. No ceiling: walking in after a
  // night, all of yesterday's work is the point. Then the floor, which age has
  // no vote on: unread work waiting on you, and diffs nobody has reviewed.
  //
  // "Alive" is deliberately NOT "non-terminal": a review that has been sitting
  // for a week is not running, and treating it as such would both bury today's
  // work and leave the backfill below with nothing to do. What saves that review
  // is the floor, and only while it is still unread.
  for (const entry of candidates) {
    const finishedToday = entry.changedAt !== null && nowMs - entry.changedAt <= DAY_MS;
    if (entry.working || finishedToday || mustShow(entry)) take(entry);
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
 * above it, for one project or — with no `projectId` — for every project at
 * once. Ordered by name, since this is a place to look something up rather than
 * a feed, or by recency when the caller asks.
 */
export function railWorktrees({ worktrees = [], entries = [], projectId = null, by = "name" } = {}) {
  const shown = new Set(entries.filter((e) => e.kind === "worktree").map((e) => e.id));
  const rows = worktrees
    .filter((w) => (projectId === null || w.project_id === projectId) && !shown.has(w.worktree_id))
    .map(worktreeEntry);
  return by === "recent"
    ? rows.sort((a, b) => (ms(b.attention?.resume_at) ?? 0) - (ms(a.attention?.resume_at) ?? 0))
    : rows.sort((a, b) => nameOf(a).localeCompare(nameOf(b)));
}
