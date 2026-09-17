// The one place a work item's entity id is derived.
//
// A branch row is stored as a run when Build cut it and as a bare checkout when
// it did not, and an issue is its own entity — so the feed's rows name several
// ids and no unified one. `entity.seen` and `entity.mute` take exactly one id,
// and every caller has to agree on which. This module is that agreement; nobody
// else picks a field.

/**
 * The entity id behind a board.list row, or null when the row names none — a
 * bare checkout nobody has claimed holds no conversation and no read cursor.
 * Null does not mean the row has no verbs: clearing an entity-less row names it
 * by what it is instead (project + branch), which core/inbox.js dismissParamsOf
 * derives.
 *
 * An issue row is its issue even when it carries a run id: the issue returns to
 * the feed only once its implementation is terminal, and it still names that
 * run. The entry is the issue.
 */
export function entityIdOf(row) {
  if (!row) return null;
  // A row that names its entity outright is that entity: the board's rows for
  // lifecycle verbs in flight (`board.list`'s `pending`) carry the id their
  // record will settle under, before any of the fields below exist.
  if (row.entity_id) return row.entity_id;
  // A capture is not an entity: it holds no conversation, keeps no read cursor,
  // and names the issue it was routed to — which belongs to the issue's own row.
  if (row.kind === "capture") return null;
  if (row.kind === "issue") return row.issue_id || null;
  return row.run_id || row.worktree_id || row.issue_id || null;
}
