// Closing out a branch — the branch surface's Done.
//
// Work ends, and on a branch it ends by deletion: Done deletes the branch, its
// checkout, and the records Build kept about it. There is no "archive the
// checkout, keep the branch" any more — that left a branch behind for the inbox
// to go on carrying, which is the one thing Done is supposed to end — so this is
// one verb with one behavior.
//
// It is never refused. What the deletion would cost travels with the row as
// `finish.warnings` (uncommitted work, commits the remote does not have, commits
// the base branch does not have), the confirmation puts those above the outline
// of what will happen, and the user decides.
//
// The inbox row's Done is the same verb from the list side (core/inboxView.js),
// and says the same words (core/inbox.js branchDoneConfirm) — one wording for
// one rule, wherever the user meets it.
//
// Pure — no DOM, no RPC. views/branchView.js renders what these return and
// makes the calls. Branch names come from the repo and are escaped by whoever
// paints them.

import { branchDoneConfirm } from "./inbox.js";

/** The `branch.finish` action each option sends. `delete` takes the branch with
 *  the checkout, which is what Done means. */
const BRANCH_FINISH_ACTION = {
  finish_delete: "delete",
};

/** The verb's options. One behavior, so the split button renders a plain
 *  button; the description carries the raw branch name and is escaped by the
 *  split button. */
function branchFinishOptions(branch) {
  return [
    {
      id: "finish_delete",
      label: "Done",
      menuLabel: "Done — delete the branch",
      description: `delete branch ${branch || "this checkout"} and its checkout`,
      busyLabel: "deleting…",
      danger: true,
    },
  ];
}

/**
 * The close-out control for one `branch.get` row: `{ shown, options }`.
 *
 * `shown` false hides it outright — a branch no checkout on this device carries
 * has nothing to delete, and a project's primary checkout IS the repository, so
 * there is nothing to file away and everything to lose. `can_finish` is the
 * bridge's own answer to "is there anything here to finish at all"; it is a
 * structural fact, never a judgement about the state of the work, so a shown
 * control is always ready to press.
 */
export function branchCloseout(row) {
  const hidden = { shown: false, options: [] };
  if (!row || row.primary || !row.can_finish) return hidden;
  if (!row.run_id && !row.worktree_id) return hidden;
  return { shown: true, options: branchFinishOptions(row.branch) };
}

/** The `branch.finish` params one option sends. */
export function branchFinishParams(optionId, { projectId, branch }) {
  const action = BRANCH_FINISH_ACTION[optionId];
  if (!action) throw new Error(`unknown branch finish option: ${optionId}`);
  return { project_id: projectId, branch, action };
}

/** The four facts Done speaks about, read off a `branch.get` row: which branch,
 *  the issue it implements, whether the work landed, and what the bridge says
 *  the deletion would cost. `fallbackBranch` names the row the URL asked for,
 *  for a read that has not answered yet. */
export function branchFinishFacts(row, fallbackBranch) {
  return {
    branch: (row && row.branch) || fallbackBranch || null,
    issueId: (row && row.issue_id) || null,
    merged: !!row && row.state === "merged",
    warnings: ((row && row.finish && row.finish.warnings) || []).map((warning) => warning.message),
  };
}

/** The confirmation for one close-out: the bridge's warnings, then the concrete
 *  steps, so the user confirms what will really happen. */
export function branchFinishConfirm(facts) {
  return branchDoneConfirm(facts);
}
