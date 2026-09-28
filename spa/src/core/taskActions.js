// Pure decisions for the adopted-run review's actions and the legacy plan
// page's doc panes. Kept side-effect-free so the views just render what these
// return, and the bridge-contract rules (the merge variants of run.git_action,
// run.abandon, and the `merge_failed:` error prefix) are unit-testable in
// isolation.

/** Whether a plan doc (the single task.doc, or a stage's task.stage_doc) is
 *  worth fetching on this pass. Docs that predate canonical storage (docsAvailable
 *  false) can never load, and a doc whose read already errored is latched off
 *  until the user re-navigates — either case skips the fetch instead of retrying
 *  it forever (the bug that pinned the pane on "loading…"). Pure. */
export function shouldFetchPlanDoc({ docsAvailable, errorLatched }) {
  return docsAvailable !== false && !errorLatched;
}

/** What a plan doc pane should render, from availability + fetch outcome:
 *  "unavailable" — predates canonical storage (an honest empty state, no retry);
 *  "error" — a read errored and is latched (an error state, no retry until the
 *  user re-navigates); "ready" — contents in hand; "loading" — still awaiting the
 *  first successful read. Unavailability wins over a latched error which wins over
 *  stale contents. Pure so the decision is testable apart from the DOM. */
export function planDocPaneState({ docsAvailable, errorLatched, hasContents }) {
  if (docsAvailable === false) return "unavailable";
  if (errorLatched) return "error";
  if (hasContents) return "ready";
  return "loading";
}

// ---- Confirmation plans (the modal-confirm step outlines) --------------------
// Each builder returns { title, intro?, actions, confirmLabel, danger } for
// core/confirm.js's confirmAction — the ordered `actions` list is the concrete
// outline of what the bridge will do, so the user confirms the real steps, not
// a vague verb. Pure so the exact copy is unit-testable.

/** The confirmation plan for a merge variant of run.git_action, or null for the
 *  non-destructive actions (commit, push) which get no modal. `base` may be a
 *  placeholder like "the base branch" when the real base is unknown. */
export function gitActionConfirm(optionId, { branch, base }) {
  const commitAndMerge = [`Commit any uncommitted changes on ${branch}`, `Merge ${branch} into ${base}`];
  const pruneSteps = [...commitAndMerge, "Delete the worktree", `Delete branch ${branch}`];
  switch (optionId) {
    case "merge_prune":
      return { title: `Merge ${branch}?`, actions: pruneSteps, confirmLabel: "Merge & clean up", danger: true };
    case "merge_keep":
      return {
        title: `Merge ${branch}?`,
        actions: [...commitAndMerge, "Keep the worktree and branch"],
        confirmLabel: "Merge",
        danger: false,
      };
    case "merge_release":
      return {
        title: `Merge ${branch}?`,
        actions: [...commitAndMerge, "Release the task — keep the worktree and branch, drop the task"],
        confirmLabel: "Merge & release",
        danger: false,
      };
    case "merge_push":
      return {
        title: `Merge ${branch}?`,
        actions: [...pruneSteps, `Push ${base} to origin`],
        confirmLabel: "Merge & push",
        danger: true,
      };
    default:
      return null;
  }
}

/** The confirmation plan for run.abandon. An adopted worktree holds files Build
 *  did not create — the outline says so explicitly. */
export function abandonConfirm({ adopted, branch }) {
  return {
    title: adopted ? "Delete this adopted worktree?" : "Abandon this task?",
    actions: [
      `Delete the worktree${adopted ? " (files Build did not create)" : ""}`,
      `Delete branch ${branch}`,
      "Keep the task as history",
    ],
    confirmLabel: adopted ? "Delete worktree" : "Abandon",
    danger: true,
  };
}

/** The human-readable reason from a `merge_failed:<reason>` error message, or
 *  null when the message is some other error. Lets the view show just the reason
 *  (conflict files, wrong base checkout) without the machine prefix. */
export function mergeFailureReason(message) {
  if (typeof message !== "string") return null;
  const prefix = "merge_failed:";
  if (!message.startsWith(prefix)) return null;
  return message.slice(prefix.length).trim();
}
