// Closing out a branch — the branch surface's Done.
//
// Work ends. A branch whose work is committed and pushed has two endings: the
// checkout is filed away and the branch kept (the base branch or the remote
// already holds the work), or the checkout and the branch both go. Both are the
// bridge's `branch.finish`, differing only in the action they pass — one verb,
// two behaviors, which is a split button (the standing rule), each behind the
// modal that outlines exactly what will happen.
//
// The inbox row's Done is the same verb from the list side (core/inboxView.js);
// this is it from inside the branch, where a reader who never opens the inbox
// can find it.
//
// Pure — no DOM, no RPC. views/branchView.js renders what these return and
// makes the calls. Branch names come from the repo and are escaped by whoever
// paints them.

import { unlinkDisclosure } from "./inbox.js";

/** The `branch.finish` action each option sends. `cleanup` removes the checkout
 *  and leaves the branch; `delete` removes the branch with it. */
const BRANCH_FINISH_ACTION = {
  finish_cleanup: "cleanup",
  finish_delete: "delete",
};

/** The split button's options, default first. Descriptions carry the raw branch
 *  name — the split button escapes them. */
function branchFinishOptions(branch) {
  const name = branch || "the branch";
  return [
    {
      id: "finish_cleanup",
      label: "Done",
      menuLabel: "Done — keep the branch",
      description: `archive this checkout, keep branch ${name}`,
      busyLabel: "finishing…",
    },
    {
      id: "finish_delete",
      menuLabel: "Done & delete branch",
      description: `archive this checkout and delete branch ${name}`,
      busyLabel: "deleting…",
    },
  ];
}

/** Why the bridge will not finish this branch yet, as human copy. Ordered the
 *  way `branch_can_finish` reads it: nothing uncommitted, somewhere to push to,
 *  nothing left to push. */
export function branchFinishBlockReason(row) {
  const stat = (row && row.stat) || {};
  const uncommitted = (stat.uncommitted && stat.uncommitted.files_changed) || 0;
  if (uncommitted > 0)
    return `Commit the ${uncommitted} uncommitted file${uncommitted === 1 ? "" : "s"} before closing this branch out.`;
  if (!stat.upstream) return "Push this branch to a remote before closing it out.";
  if (stat.ahead) return `Push the ${stat.ahead} unpushed commit${stat.ahead === 1 ? "" : "s"} before closing this branch out.`;
  return "This branch still carries work that exists only on this device.";
}

/**
 * The close-out control for one `branch.get` row:
 *   { shown, ready, reason, options }
 *
 * `shown` false hides it outright — a branch no checkout on this device carries
 * has nothing to close, and a project's primary checkout IS the repository, so
 * there is nothing to file away and everything to lose (the bridge refuses it).
 * `ready` follows the row's own `can_finish`, so the button offers what will
 * actually succeed; when it is false, `reason` says what stands in the way.
 */
export function branchCloseout(row) {
  const hidden = { shown: false, ready: false, reason: "", options: [] };
  if (!row || row.primary) return hidden;
  if (!row.run_id && !row.worktree_id) return hidden;
  const ready = !!row.can_finish;
  return {
    shown: true,
    ready,
    reason: ready ? "" : branchFinishBlockReason(row),
    options: branchFinishOptions(row.branch),
  };
}

/** The `branch.finish` params one option sends. `unlink` is the override the
 *  bridge's own refusal names: finish the branch, leave the issue open. */
export function branchFinishParams(optionId, { projectId, branch, unlink = false }) {
  const action = BRANCH_FINISH_ACTION[optionId];
  if (!action) throw new Error(`unknown branch finish option: ${optionId}`);
  const params = { project_id: projectId, branch, action };
  return unlink ? { ...params, unlink: true } : params;
}

/** The confirmation plan for one option — the concrete steps, in order, so the
 *  user confirms what the bridge will really do. */
export function branchFinishConfirm(optionId, { branch, issueId = null }) {
  const name = branch || "this checkout";
  const deleting = optionId === "finish_delete";
  const actions = [`Remove the checkout for ${name}`, deleting ? `Delete branch ${name}` : `Keep branch ${name}`];
  if (issueId) actions.push("Archive the issue it implements, with its stage plans");
  return {
    title: deleting ? `Close out ${name} and delete the branch?` : `Close out ${name}?`,
    intro: "Its work is committed and pushed.",
    actions,
    confirmLabel: deleting ? "Done & delete branch" : "Done",
    danger: deleting,
  };
}

/** The refusal that IS the disclosure: the bridge will not archive an issue its
 *  branch has not implemented, and its error names the way past it. */
export function isUnlinkRefusal(message) {
  return typeof message === "string" && message.includes("unlink");
}

/** That disclosure, as a confirmation. The copy belongs to the inbox's Done —
 *  one wording for one rule, wherever the user meets it. */
export function branchUnlinkConfirm(branch, message) {
  return unlinkDisclosure({ branch }, message);
}
