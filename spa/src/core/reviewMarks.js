// What the reviewer has marked on the files of a changeset.
//
// Two marks, and they mean different things. APPROVED is a verdict: this file
// is read and settled, and saying so collapses it, which is what makes the
// stack shorten as the reviewer works down it. SELECTED is a hand on a row:
// files gathered so one verb can be aimed at all of them, and so a commit can
// be narrowed to what the reviewer actually meant.
//
// Both live here rather than in whichever renderer happens to be drawing the
// stack, because the Changes surface draws its changesets two ways — its own
// stacks, and the review plug's aggregate — and a mark made in one has to be
// the same mark in the other.

/** One reviewer's marks over one surface's files. */
export function createReviewMarks() {
  const approved = new Set();
  const selected = new Set();
  const flip = (set, path) => (set.has(path) ? set.delete(path) : set.add(path));
  return {
    approved,
    selected,
    toggleApproved: (path) => flip(approved, path),
    toggleSelected: (path) => flip(selected, path),
    /** Approve everything selected, and let the selection go: the verb has been
     *  aimed and fired, and a selection left standing is a second verb waiting
     *  to land on files the reviewer has moved on from. */
    approveSelected() {
      for (const path of selected) approved.add(path);
      selected.clear();
    },
    clearSelection: () => selected.clear(),
  };
}

/**
 * The paths a commit takes: the files the reviewer ticked, or every changed
 * file when they ticked none.
 *
 * Ticking nothing is the ordinary case and means the whole worktree — the
 * reviewer who wants a narrower commit says so by ticking, and is never made to
 * tick everything to get what they would have got anyway.
 */
export function commitPaths(changedPaths, selected) {
  const ticked = (changedPaths || []).filter((path) => selected && selected.has(path));
  return ticked.length ? ticked : [...(changedPaths || [])];
}
