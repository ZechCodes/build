// The task's review surface — the Changes rail's "All changes" entry: the
// aggregate run.diff against the base branch. Everything about drawing a
// changeset, holding pending comments, and posting them as anchored messages
// lives in core/changesReview.js, which every review surface shares; what is
// here is only what belongs to a task: where its diff comes from, that its
// comments go to the coding agent through run.request_changes, and the
// lifecycle verbs (merge, commit, push) its actionbar offers.
//
// The plug instance (and its pending comments) belongs to the task view, so
// remounts — tab switches, shell rebuilds — keep review state.

import { createAgentSelection } from "../core/agentSelection.js";
import { createReviewPlug, REVIEW_POLL_MS } from "../core/changesReview.js";
import { mountSplitButton, createSingleFlight } from "../core/splitButton.js";
import { currentRevisionId } from "../core/thread.js";
import { mergeFailureReason, gitActionConfirm } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError } from "../core/notify.js";

export { REVIEW_POLL_MS };

// How long a git action's result (Committed./Pushed.) stays in the hint.
const FLASH_MS = 6000;

// Each option id maps to a run.git_action call. cleanup is omitted for
// commit/push (the bridge rejects cleanup on non-merges).
const GIT_ACTION_RPC = {
  merge_prune: { action: "merge", cleanup: "prune" },
  merge_keep: { action: "merge", cleanup: "keep" },
  merge_release: { action: "merge", cleanup: "release" },
  merge_push: { action: "merge_push", cleanup: "prune" },
  commit: { action: "commit" },
  push: { action: "push" },
};

// The states whose surface takes comments: the agent is there to read them.
const COMMENTABLE_STATES = ["review", "building"];

// The merge option set. Adopted tasks add "Merge & release" (un-adopt after
// merge, keeping the user's worktree). A primary run (adopted around the repo
// root) narrows to commit and push: the bridge refuses merging the primary
// checkout — its branch is what a merge would target. Descriptions carry the
// raw base branch — the split button escapes them.
export function reviewMergeOptions(adopted, base, primary = false) {
  const commitAndPush = [
    { id: "commit", label: "Commit", menuLabel: "Commit", description: "commit the work, stay on the branch", busyLabel: "committing…" },
    { id: "push", menuLabel: "Push", description: "commit, then push this branch to origin", busyLabel: "pushing…" },
  ];
  if (primary) return commitAndPush;
  const options = [
    { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: `commit, merge into ${base}, remove the worktree + branch`, busyLabel: "merging…" },
    { id: "merge_keep", menuLabel: "Merge & keep worktree", description: `merge into ${base}, keep the worktree and branch`, busyLabel: "merging…" },
  ];
  if (adopted)
    options.push({ id: "merge_release", menuLabel: "Merge & release", description: `merge into ${base}, then un-adopt — keep the worktree and branch, drop the task`, busyLabel: "merging…" });
  options.push({ id: "merge_push", menuLabel: "Merge & push", description: `merge, then push ${base} to origin`, busyLabel: "merging & pushing…" }, ...commitAndPush);
  return options;
}

/** The resting copy under a reviewable task's diff: what there is to do here.
 *  The primary checkout has no worktree to finish, so it says what its own
 *  actions actually do. */
export function reviewHint(task) {
  return task && task.primary
    ? "Select code or click a line number to comment, or commit the work."
    : "Select code or click a line number to comment, or finish the worktree.";
}

/**
 * createTaskReview({ taskId, callRpc, getTask, isOffline, agentSelection,
 *   onMerged }) → { mount(host), unmount() } — the gitPane review plug for a
 * task.
 *
 * getTask() returns the task view's freshest run payload. The conversation
 * belongs to the agent that owns it; this plug owns only the diff, the pending
 * review comments, and the review git actions. `agentSelection` says which of
 * the branch's agents is being reviewed — the one whose bubble is open — so the
 * comments land in the conversation the reviewer was reading.
 */
export function createTaskReview({ taskId, callRpc, getTask, isOffline, agentSelection = createAgentSelection(), onMerged }) {
  // ONE single-flight latch for the git split button, owned by the plug — not
  // by each repaint. Without a shared latch, mid-merge the poll would replace
  // the disabled "merging…" button with an enabled Merge that can dispatch a
  // second concurrent run.git_action. It is also the freeze key: while active,
  // the actionbar is left untouched.
  const gitFlight = createSingleFlight();
  let flashMessage = ""; // a recent git-action result, outliving the poll

  const plug = createReviewPlug({
    isOffline,
    fetchDiff: async () => {
      if (!getTask()) return null;
      const diff = await callRpc("run.diff", { run_id: taskId });
      // Re-read AFTER the round trip: a paint that snapshotted the task before
      // awaiting would render pre-post state if something landed underneath it.
      const task = getTask();
      if (!task) return null;
      return {
        patch: diff.patch,
        key: task.state,
        commentable: COMMENTABLE_STATES.includes(task.state),
        // Review prioritization: the run's own diff is the one the triage pass
        // read, so this is where its ordering belongs. Null until a pass lands —
        // the stack says it is untriaged rather than implying it was read.
        triage: task.triage || null,
        projectId: task.project_id || null,
      };
    },
    submit: (messages) => callRpc("run.request_changes", { run_id: taskId, ...agentSelection.scope(), messages }),
    revisionId: () => currentRevisionId(getTask()?.thread, "diff"),
    statusHtml: () =>
      getTask() && getTask().state === "building"
        ? '<span class="dim live-claim">● coding agent working — diff updating live…</span>'
        : "",
    actionsFrozen: () => gitFlight.active(),
    renderIdleActions: (actions, hintHost) => {
      // A git action (or its confirm modal) is in flight: leave the actionbar
      // exactly as it is, so no repaint can remount an enabled button under the
      // pending RPC (or pop a second modal).
      if (gitFlight.active()) return true;
      const task = getTask();
      const state = task && task.state;
      if (state === "building") {
        hintHost.textContent = "Comment on the diff to request changes — even while the agent is working.";
        actions.innerHTML = "";
        return true;
      }
      if (state !== "review") return false;
      hintHost.textContent = flashMessage || reviewHint(task);
      mountSplitButton(actions, {
        options: reviewMergeOptions(task.adopted, task.base_branch || "main", task.primary),
        run: (optionId) => runGitAction(optionId, task),
        flight: gitFlight,
      });
      return true;
    },
  });

  const flash = (message) => {
    flashMessage = message;
    setTimeout(() => {
      flashMessage = "";
      plug.refreshActions();
    }, FLASH_MS);
  };

  /** One review git action: confirm the decisive ones, run it, then either hand
   *  the surface off (a merge leaves it) or say what happened. */
  const runGitAction = async (optionId, task) => {
    const { action, cleanup } = GIT_ACTION_RPC[optionId];
    // Merge variants are decisive: confirm with the exact step outline first. A
    // cancel throws BEFORE any RPC — the split button restores the primary, and
    // no error notice appears.
    const confirmPlan = gitActionConfirm(optionId, { branch: task.branch || "the branch", base: task.base_branch || "main" });
    if (confirmPlan && !(await confirmAction(confirmPlan))) throw new Error("cancelled");
    flashMessage = "";
    const params = { run_id: taskId, action };
    if (cleanup) params.cleanup = cleanup;
    try {
      await callRpc("run.git_action", params);
    } catch (e) {
      // Failures persist as an expandable notice (the full message in the
      // detail); successes stay transient in the hint.
      const reason = mergeFailureReason(e.message);
      notifyError(reason ? "Merge failed: " + reason.split("\n")[0] : "Action failed", e.message);
      throw e; // let the split button restore the primary button
    }
    if (action === "merge" || action === "merge_push") {
      onMerged();
      return;
    }
    flash(action === "commit" ? "Committed." : "Pushed " + (task.branch || "branch") + ".");
  };

  return {
    mount: (element) => plug.mount(element),
    unmount: () => plug.unmount(),
  };
}
