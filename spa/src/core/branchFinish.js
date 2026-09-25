// Closing out a branch — the branch surface's Done.
//
// Work ends, and on a branch it ends by deletion: Done deletes the branch, its
// checkout, and the records Build kept about it. There is no "archive the
// checkout, keep the branch" any more — that left a branch behind for the inbox
// to go on carrying, which is the one thing Done is supposed to end — so this is
// one verb with one behavior.
//
// Deleting the branch is the bridge's to do, and only a bridge announcing
// `branches.finishDelete` does it (#87); an older one removes the checkout and
// keeps the branch whatever it is sent. So every function here that promises
// or asks for the deletion takes `deletesBranch` — what the cache holds for
// that machine (core/branchDeleteSupport.js) — and on an older bridge the
// options and the params say only what will really happen.
//
// What the deletion would cost travels with the row as `finish.warnings`
// (uncommitted work, commits the remote does not have, commits the base branch
// does not have), and the confirmation puts those above the outline of what
// will happen. They are advance notice, not a veto: the bridge is the one that
// decides, and it refuses Done while the work is still only in the checkout
// (planning/v2/workspaces.md, "Finish and retention"). A refusal comes back as
// the row's error, in the bridge's own words.
//
// The inbox row's Done is the same verb from the list side (core/inboxView.js),
// and says the same words (core/inbox.js branchDoneConfirm) — one wording for
// one rule, wherever the user meets it.
//
// Pure — no DOM, no RPC. views/branchView.js renders what these return and
// makes the calls. Branch names come from the repo and are escaped by whoever
// paints them.

import { branchDoneConfirm, entryKeyOf } from "./inbox.js";

export const BRANCH_DONE_OPTION = "finish_delete";

/** The `branch.finish` action each option sends where the bridge deletes the
 *  branch. `delete` takes the branch with the checkout, which is what Done
 *  means. */
const BRANCH_FINISH_ACTION = {
  [BRANCH_DONE_OPTION]: "delete",
};

/** The verb's option, by whether the bridge deletes the branch. The
 *  description carries the raw branch name and is escaped by the split
 *  button. */
const DONE_OPTION = {
  deletes: (branch) => ({
    menuLabel: "Done — delete the branch",
    description: `delete branch ${branch || "this checkout"} and its checkout`,
    busyLabel: "deleting…",
  }),
  keeps: (branch) => ({
    menuLabel: "Done — remove the checkout",
    description: `remove the checkout of ${branch || "this branch"}; the branch stays`,
    busyLabel: "removing…",
  }),
};

/** The verb's options. One behavior, so the split button renders a plain
 *  button. */
function branchFinishOptions(branch, deletesBranch) {
  const words = DONE_OPTION[deletesBranch ? "deletes" : "keeps"](branch);
  return [{ id: BRANCH_DONE_OPTION, label: "Done", ...words, danger: true }];
}

/**
 * The close-out control for one `branch.get` row: `{ shown, options }`.
 *
 * `shown` false hides it outright — a branch no checkout on this device carries
 * has nothing to delete. `can_finish` is the
 * bridge's own answer to "is there anything here to finish at all"; it is a
 * structural fact, never a judgement about the state of the work, so a shown
 * control is always ready to press.
 */
export function branchCloseout(row, { deletesBranch = false } = {}) {
  const hidden = { shown: false, options: [] };
  if (!row || !row.can_finish) return hidden;
  if (!row.run_id && !row.worktree_id) return hidden;
  return { shown: true, options: branchFinishOptions(row.branch, deletesBranch) };
}

/** The `branch.finish` params one option sends. The action goes only to a
 *  bridge that deletes the branch: an older one would take it and drop it. */
export function branchFinishParams(optionId, { projectId, branch, deletesBranch = false }) {
  const action = BRANCH_FINISH_ACTION[optionId];
  if (!action) throw new Error(`unknown branch finish option: ${optionId}`);
  return deletesBranch ? { project_id: projectId, branch, action } : { project_id: projectId, branch };
}

export function branchFinishFailureSummary(name) {
  return `Couldn't finish ${name || "this item"}`;
}

/** A post-removal branch outcome that needs a lasting notice. A refusal kept
 *  the branch; a recovery warning means the checkout went but restoration
 *  failed, so its summary must never claim the branch stayed. */
export function branchFinishNotice(name, answer) {
  if (answer?.branch_deleted !== false && !answer?.branch_reason) return null;
  return {
    summary: answer.branch_deleted === true
      ? `Removed the checkout of ${name || "this branch"}; branch recovery failed`
      : `Removed the checkout of ${name || "this branch"}; the branch stays`,
    detail: String(answer.branch_reason || ""),
  };
}

/** The name the inbox is holding this branch's row under. A row with no entity
 *  of its own is named by its project, and a project is only named once the
 *  device is said with it — a `branch.get` answer is one device's and carries
 *  no such name, so the caller says which project key this surface is on. */
export function branchInboxKey(row, { projectId, branch, projectKey }) {
  return entryKeyOf({
    ...row,
    kind: "branch",
    project_id: (row && row.project_id) || projectId,
    projectKey: (row && row.projectKey) || projectKey,
    branch: (row && row.branch) || branch,
  });
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
