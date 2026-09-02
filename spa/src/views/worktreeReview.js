// A worktree's review surface — the merge-base-anchored diff of everything the
// worktree carries, mounted as the Changes rail's "All changes" entry. It is
// the task surface's plug with a different source and different verbs: the
// drawing, the pending comments, and the anchored posts are the shared ones
// (core/changesReview.js).
//
// What is this surface's own is adoption: sending comments — like any Merge or
// Abandon from its actionbar — transparently binds the worktree to a task
// first, and hands the user off to it. Everything rendered from the worktree's
// branch, base and path is UNTRUSTED and escaped by the renderers.
//
// The plug instance belongs to the worktree view, so remounts (tab switches,
// the rail selection moving away and back) keep pending review comments.

import { createReviewPlug, REVIEW_POLL_MS } from "../core/changesReview.js";
import { mountSplitButton } from "../core/splitButton.js";
import { gitActionConfirm, abandonConfirm, mergeFailureReason } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError } from "../core/notify.js";

export const WORKTREE_REVIEW_POLL_MS = REVIEW_POLL_MS;

// The adopted-task merge set for the browse view: prune / keep / release only.
// Committing and pushing this worktree is what the Changes surface around this
// plug is for.
const WORKTREE_MERGE_OPTIONS = [
  { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: "commit, merge into the base branch, remove the worktree + branch", busyLabel: "merging…" },
  { id: "merge_keep", menuLabel: "Merge & keep worktree", description: "merge into the base branch, keep the worktree and branch", busyLabel: "merging…" },
  { id: "merge_release", menuLabel: "Merge & release", description: "merge into the base branch, then un-adopt — keep the worktree and branch, drop the task", busyLabel: "merging…" },
];
const MERGE_RPC = {
  merge_prune: { action: "merge", cleanup: "prune" },
  merge_keep: { action: "merge", cleanup: "keep" },
  merge_release: { action: "merge", cleanup: "release" },
};

/** Why a worktree cannot be adopted, said to the reader. A detached HEAD and
 *  the base branch itself are both "there is no feature branch here to take
 *  over" — they only differ in what to do about it. */
export function unadoptableHint(meta) {
  return meta && meta.branch
    ? "This is the base branch — check out a feature branch to adopt."
    : "Detached HEAD — check out a branch to adopt.";
}

/**
 * createWorktreeReview({ projectId, worktreeId, callRpc, adopting, ... }) →
 *   { mount(host), unmount(), getBase() } — the gitPane review plug.
 *
 * `adopting` is the view's shared createAdoptingCall: the first mutating action
 * binds the worktree to a task, and every caller must agree on which task that
 * is. `onAdopted()` hands off to it, `onFinished()` leaves the surface after a
 * merge or abandon, and `onGone()` fires when the worktree stops resolving
 * without this surface having adopted it.
 */
export function createWorktreeReview({
  projectId,
  worktreeId,
  callRpc,
  adopting,
  initialProvider = "",
  isOffline = () => false,
  onAdopted = () => {},
  onFinished = () => {},
  onGone = () => {},
  openFile = null,
}) {
  // A mutating action (request changes / merge / abandon) is running: it is
  // about to adopt or remove this worktree, so the poll must not race it to a
  // "the worktree vanished" verdict.
  let acting = false;
  let meta = null; // the last worktree.diff payload's branch/base/adoptable/path

  const branchLabel = () => (meta && meta.branch) || "the branch";
  const baseLabel = () => (meta && meta.base_branch) || "main";

  /** Run a mutating action with the poll held off: after it, worktree.diff
   *  resolves to "unknown worktree_id" — a poll landing mid-action would read
   *  that as the worktree having vanished. */
  const act = async (work) => {
    acting = true;
    try {
      return await work();
    } finally {
      acting = false;
    }
  };

  /** Every mutation here adopts first. A provider chosen for THIS worktree — in
   *  the sheet, before the directory existed — is the agent the human asked to
   *  run here, so adoption dispatches with it; otherwise the account default. */
  const adopt = (method, params) => {
    adopting.setAdoptParams(initialProvider ? { provider: initialProvider } : {});
    return adopting.runCall(method, params);
  };

  const plug = createReviewPlug({
    isOffline,
    openFile,
    // Until this worktree is adopted it is its own entity; once it is, the run
    // it became is the one the bridge names.
    entity: () => adopting.adoptedRunId() || worktreeId,
    fetchDiff: async () => {
      if (acting) return null;
      // Adopted already? The worktree lives on as a task now — never poll it
      // (the diff would 404) — hand off so its outcome is where the user can
      // see it.
      if (adopting.adoptedRunId()) {
        onAdopted();
        return null;
      }
      let res;
      try {
        res = await callRpc("worktree.diff", { project_id: projectId, worktree_id: worktreeId });
      } catch (e) {
        if (String(e && e.message).includes("unknown worktree_id") && !acting) onGone();
        return null; // otherwise transient — the poll retries
      }
      meta = { branch: res.branch, base_branch: res.base_branch, path: res.path, adoptable: res.adoptable };
      // A non-adoptable worktree (detached HEAD, or the base branch itself)
      // only browses: there is no task for a comment to reach.
      return { patch: res.patch, key: String(res.adoptable), commentable: Boolean(res.adoptable) };
    },
    submit: async (messages) => {
      try {
        await act(() => adopt("run.request_changes", { messages }));
      } catch (e) {
        // Adoption succeeded but the follow-up failed: the task now owns this
        // worktree and its error — hand off rather than stranding the user on a
        // surface whose polls are about to stop resolving.
        if (!adopting.adoptedRunId()) throw e;
      }
      onAdopted();
    },
    hint: "Your comments adopt this worktree as a task and are sent to the coding agent.",
    actionsFrozen: () => acting,
    renderIdleActions: (actions, hintHost) => {
      const adoptable = Boolean(meta && meta.adoptable);
      // The hint speaks on every tick — the comment tray borrows this line while
      // comments are pending and the bar has to take it back.
      hintHost.textContent = adoptable
        ? "Comment on the diff to request changes, or finish the worktree."
        : unadoptableHint(meta);
      // The buttons do not. This runs on every 1.6s poll, and rebuilding the bar
      // would take the merge menu the reviewer just opened — and the button a
      // press is landing on — with it. What the bar offers turns on one thing,
      // so a tick that says the same thing leaves the bar alone.
      const wanted = adoptable ? "finish" : "browse";
      if (actions.dataset.worktreeActions === wanted) return true;
      actions.dataset.worktreeActions = wanted;
      actions.innerHTML = "";
      if (!adoptable) return true;
      const mergeHost = document.createElement("span");
      const abandon = document.createElement("button"); // quiet — the confirm guards it
      abandon.className = "btn";
      abandon.textContent = "Abandon & delete";
      actions.appendChild(mergeHost);
      actions.appendChild(abandon);
      mountSplitButton(mergeHost, { options: WORKTREE_MERGE_OPTIONS, run: runMerge });
      abandon.onclick = () => runAbandon(abandon);
      return true;
    },
  });

  const runMerge = async (optionId) => {
    const { action, cleanup } = MERGE_RPC[optionId];
    // Confirm the exact step outline BEFORE adopting — a cancel leaves the
    // surface live and untouched.
    const confirmPlan = gitActionConfirm(optionId, { branch: branchLabel(), base: baseLabel() });
    if (confirmPlan && !(await confirmAction(confirmPlan))) throw new Error("cancelled");
    try {
      await act(() => adopt("run.git_action", { action, cleanup }));
    } catch (e) {
      // A merge failure persists as an expandable notice (the full message in
      // the detail).
      const reason = mergeFailureReason(e.message);
      notifyError(reason ? "Merge failed: " + reason.split("\n")[0] : "Action failed", e.message);
      // Adopted, then the merge failed (a conflict is the common case for a
      // stale external worktree): hand off to the task holding merge_failed.
      if (adopting.adoptedRunId()) {
        onAdopted();
        return;
      }
      throw e; // let the split button restore its primary
    }
    onFinished();
  };

  const runAbandon = async (button) => {
    // adopted: true — this surface deletes files Build did not create.
    if (!(await confirmAction(abandonConfirm({ adopted: true, branch: branchLabel() })))) return;
    button.disabled = true;
    button.textContent = "abandoning…";
    try {
      await act(() => adopt("run.abandon", {}));
    } catch (e) {
      if (adopting.adoptedRunId()) {
        onAdopted();
        return;
      }
      notifyError("Abandon failed", e.message);
      button.disabled = false;
      button.textContent = "Abandon & delete";
      return;
    }
    onFinished();
  };

  return {
    /** The branch the rail's "All changes" entry names this diff against. */
    getBase: () => baseLabel(),
    mount: (element) => plug.mount(element),
    unmount: () => plug.unmount(),
  };
}
