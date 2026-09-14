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
import { currentRevisionId, MUTATION_THREAD_PAGE } from "../core/thread.js";
import { mergeFailureReason, gitActionConfirm } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError, notifySuccess } from "../core/notify.js";
import { coordinatedRead, rpcReadKey } from "../core/readRequests.js";

export { REVIEW_POLL_MS };

// How long a git action's result (Committed./Pushed.) stays in the hint.

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
export function createTaskReview({
  taskId,
  callRpc,
  // The cache and the read-coordination identity of the machine the run is on.
  cacheScope = null,
  getTask,
  isOffline,
  agentSelection = createAgentSelection(),
  onMerged,
  navigate = null,
  viewingContext = null,
}) {
  // ONE single-flight latch for the git split button, owned by the plug — not
  // by each repaint. Without a shared latch, mid-merge the poll would replace
  // the disabled "merging…" button with an enabled Merge that can dispatch a
  // second concurrent run.git_action. It is also the freeze key: while active,
  // the actionbar is left untouched.
  const gitFlight = createSingleFlight();
  const requestScope = cacheScope || callRpc;

  const plug = createReviewPlug({
    cacheScope,
    isOffline,
    viewingContext,
    navigate,
    // The diff is this run's, so the run's own change events are what stale it.
    entity: taskId,
    fetchDiff: async (ifDiffKey) => {
      if (!getTask()) return null;
      const params = { run_id: taskId, ...(ifDiffKey ? { if_diff_key: ifDiffKey } : {}) };
      const diff = await coordinatedRead({
        key: rpcReadKey({
          deviceId: cacheScope?.deviceId,
          requestScope,
          repository: `run:${taskId}`,
          call: callRpc,
          method: "run.diff",
          params,
        }),
        load: () => callRpc("run.diff", params),
      });
      // Re-read AFTER the round trip: a paint that snapshotted the task before
      // awaiting would render pre-post state if something landed underneath it.
      const task = getTask();
      if (!task) return null;
      const triageEnabled = task.triage_enabled === true;
      return {
        patch: diff.patch,
        unchanged: diff.unchanged,
        diff_key: diff.diff_key,
        file_edited_at: diff.file_edited_at,
        key: task.state,
        commentable: COMMENTABLE_STATES.includes(task.state),
        // Review prioritization: the run's own diff is the one the triage pass
        // read, so this is where its ordering belongs. Null until a pass lands —
        // the stack says it is untriaged rather than implying it was read.
        ...(triageEnabled ? { triage: task.triage || null } : {}),
        triageEnabled,
        projectId: task.project_id || null,
      };
    },
    submit: (messages) => {
      const context = viewingContext?.snapshot?.();
      return callRpc("run.request_changes", {
        run_id: taskId,
        ...agentSelection.scope(),
        messages: context ? messages.map((message) => ({ ...message, viewing_context: context })) : messages,
        ...MUTATION_THREAD_PAGE,
      }).then((result) => { viewingContext?.clearSelectionIfMatches?.(context); return result; });
    },
    // The reviewer's disagreement with the pass. It lands on the run's own
    // triage, and in the conversation of the agent that wrote the rationale.
    submitOverride: ({ hunk_id, direction, note }) => {
      if (getTask()?.triage_enabled !== true) throw new Error("Review prioritization is turned off.");
      return callRpc("triage.override", { run_id: taskId, hunk_id, direction, note });
    },
    revisionId: () => currentRevisionId(getTask()?.thread, "diff"),
    statusHtml: () =>
      getTask() && getTask().state === "building"
        ? '<span class="dim live-claim">● coding agent working — diff updating live…</span>'
        : "",
    actionsFrozen: () => gitFlight.active(),
    // The merge verb, in the git toolbar above the diff. Nothing else goes
    // there: the diff says what changed, the review bar says whether the agent
    // is still writing it, and a line of resting copy under the stack telling
    // the reviewer they may click a line number was a sentence in the way.
    renderIdleActions: (actions) => {
      // A git action (or its confirm modal) is in flight: leave the bar exactly
      // as it is, so no repaint can remount an enabled button under the pending
      // RPC (or pop a second modal).
      if (gitFlight.active()) return true;
      const task = getTask();
      if (!task || task.state !== "review") return false;
      mountSplitButton(actions, {
        options: reviewMergeOptions(task.adopted, task.base_branch || "main", task.primary),
        run: (optionId) => runGitAction(optionId, task),
        flight: gitFlight,
        variant: "mini",
      });
      return true;
    },
  });

  /// What just happened, said once. It used to be a line of copy under the
  /// diff that faded after a moment; with that bar gone the notice is where a
  /// result belongs — it does not need the reviewer to be looking down there.
  const flash = (message) => notifySuccess(message);

  /** One review git action: confirm the decisive ones, run it, then either hand
   *  the surface off (a merge leaves it) or say what happened. */
  // eslint-disable-next-line complexity -- ratchet: this callback is at 12, cap 10 — reduce it, then drop this line
  const runGitAction = async (optionId, task) => {
    const { action, cleanup } = GIT_ACTION_RPC[optionId];
    // Merge variants are decisive: confirm with the exact step outline first. A
    // cancel throws BEFORE any RPC — the split button restores the primary, and
    // no error notice appears.
    const confirmPlan = gitActionConfirm(optionId, { branch: task.branch || "the branch", base: task.base_branch || "main" });
    if (confirmPlan && !(await confirmAction(confirmPlan))) throw new Error("cancelled");
    const params = { run_id: taskId, action, ...MUTATION_THREAD_PAGE };
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

  // The plug itself. This view adds the run's verbs and its diff source through
  // the options above; it has nothing of its own to put on the handle, and an
  // enumerated copy would silently drop whatever the plug learns to do next.
  return plug;
}
