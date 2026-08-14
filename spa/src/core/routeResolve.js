// Turning a pre-redesign URL into a work item.
//
// The old URLs addressed runs, worktrees, plans and primary checkouts by id.
// The new ones address a branch by (project, branch name) and an issue by
// (project, issue id), so the ids that survived need a lookup: the feed's
// items[] rows carry both halves. Pure — the caller supplies the rows.

const branchRouteFor = (row, tab) =>
  row && row.branch ? { name: "branch", projectId: row.project_id, branch: row.branch, tab: tab || "changes" } : null;

/** The rows that could be what the URL meant, most likely first: a row in the
 *  project the URL named beats one in any other project. */
const inNamedProjectFirst = (rows, projectId) =>
  projectId ? [...rows.filter((row) => row.project_id === projectId), ...rows.filter((row) => row.project_id !== projectId)] : rows;

/**
 * The new route a legacy reference points at, or null when this feed carries
 * nothing by that id (deleted, or not yet polled — the caller decides whether
 * to wait or land on the inbox).
 *
 * @param ref {kind: 'run'|'worktree'|'issue'|'primary', id?, projectId?, tab?, stage?}
 * @param items the feed's items[] rows
 */
export function resolveLegacyRoute(ref, items) {
  if (!ref) return null;
  const rows = inNamedProjectFirst(items || [], ref.projectId);
  if (ref.kind === "run") return branchRouteFor(rows.find((row) => row.run_id === ref.id), ref.tab);
  if (ref.kind === "worktree") return branchRouteFor(rows.find((row) => row.worktree_id === ref.id), ref.tab);
  // A primary checkout is named by its project alone, so the project has to
  // match — the primary row of some other project is not what the URL meant.
  if (ref.kind === "primary") {
    return branchRouteFor(
      rows.find((row) => row.primary && row.project_id === ref.projectId),
      ref.tab,
    );
  }
  if (ref.kind === "issue") {
    const row = rows.find((candidate) => candidate.issue_id === ref.id);
    if (!row) return null;
    // Dedup: while an issue's implementation is in flight the issue has no row
    // of its own — the branch row carries its id, and the branch is the nearest
    // surface the URL can open.
    if (row.kind === "branch") return branchRouteFor(row, ref.tab);
    const route = { name: "issue", projectId: row.project_id, id: ref.id };
    if (ref.stage) route.stage = ref.stage;
    return route;
  }
  return null;
}
