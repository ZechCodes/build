// The flat rail: one list of everything Build knows about, ungrouped.
//
// The per-project rail (core/sidebar.js) asks rail.js the same question once per
// project. This asks it once, for all of them — one selection, one backfill, one
// order — and adds each project's main checkout as a row of its own, since a
// main branch is where an adopted agent paints and where uncommitted changes sit
// until someone looks. Pure model only; how it looks is core/sidebar.js.

import { mainEntry, railEntries, railWorktrees } from "./rail.js";

/** The run that owns a project's primary checkout: the one the checkout names,
 *  falling back to whichever of the project's runs is flagged primary. */
function primaryRunOf(runs, primaryChange) {
  const named = primaryChange.run_id && runs.find((r) => r.run_id === primaryChange.run_id);
  return named || runs.find((r) => r.project_id === primaryChange.project_id && r.primary) || null;
}

/**
 * The flat rail's model: `{entries, worktrees, unread}`.
 *
 * `entries` are every project's runs, issues, worktrees and main checkouts in
 * one ordered list; `worktrees` is what the entries did not show, from every
 * project, for the single `Worktrees ›` fold; `unread` is the badge's count.
 *
 * Every row carries `project_name`, because without the project blocks nothing
 * else says where a row lives.
 */
export function buildAllRailModel({
  projects,
  runs,
  plans,
  externalWorktrees,
  primaryChanges,
  readIds,
  nowMs,
  pendingDone = new Set(),
}) {
  const visible = (kind, id) => !pendingDone.has(`${kind}:${id}`);
  const known = projects || [];
  const nameOfProject = new Map(known.map((p) => [p.project_id, p.name || ""]));
  const liveRuns = (runs || []).filter((r) => visible("run", r.run_id));
  const livePlans = (plans || []).filter((pl) => visible("plan", pl.plan_id));
  const liveWorktrees = (externalWorktrees || []).filter((w) => visible("worktree", w.worktree_id));

  // Each project's checkout becomes a row, carrying the dot of the run that
  // adopted it. That run is the main row, so it is never also a row of its own.
  const owners = new Set();
  const mains = known
    .map((project) => {
      const primaryChange = (primaryChanges || []).find((c) => c.project_id === project.project_id);
      if (!primaryChange) return null;
      const run = primaryRunOf(liveRuns, primaryChange);
      if (run) owners.add(run.run_id);
      return mainEntry({ project, primaryChange, run });
    })
    .filter(Boolean);

  const entries = railEntries({
    runs: liveRuns.filter((r) => !owners.has(r.run_id)),
    plans: livePlans,
    worktrees: liveWorktrees,
    mains,
    nowMs,
  });
  const worktrees = railWorktrees({ worktrees: liveWorktrees, entries });
  // The badge counts what is waiting on you and unread — the same question the
  // yellow dot answers, so the two can never disagree.
  const unread =
    liveRuns.filter((r) => r.needs_attention && !readIds.has(r.run_id)).length +
    livePlans.filter((pl) => pl.needs_attention && !readIds.has(pl.plan_id)).length;

  const named = (entry) => ({ ...entry, project_name: entry.project_name ?? nameOfProject.get(entry.project_id) ?? "" });
  return { entries: entries.map(named), worktrees: worktrees.map(named), unread };
}
