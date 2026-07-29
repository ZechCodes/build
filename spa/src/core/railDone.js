// Done controls for plans and external worktrees: pure action/confirmation
// builders plus thin, dependency-injected DOM wiring for the persistent rail.

import { esc } from "./text.js";

const branchLabel = (worktree) => worktree.branch || "the detached HEAD";
const upstreamLabel = (worktree) => worktree.upstream || "its upstream";
const plural = (count, singular, pluralForm = `${singular}s`) => `${count} ${count === 1 ? singular : pluralForm}`;

export function planArchiveConfirm() {
  return {
    title: "Archive this plan?",
    intro: "This closes the plan without deleting its artifacts.",
    actions: ["Move the plan and its documents to Project Archive"],
    confirmLabel: "Archive plan",
    danger: false,
  };
}

/** Backend-defined finish choices, ordered from preservative to destructive. */
export function worktreeFinishActions(worktree) {
  if (worktree.dirty_files === 0) {
    return [
      {
        id: "cleanup",
        label: "Clean up",
        description: worktree.branch
          ? `Remove the worktree and preserve branch ${worktree.branch}`
          : "Remove the detached worktree",
      },
    ];
  }

  const actions = [];
  if (worktree.upstream) {
    actions.push({
      id: "push",
      label: "Push",
      description: `Commit the changes and push ${branchLabel(worktree)} to ${worktree.upstream}`,
    });
  }
  if (worktree.branch && worktree.branch !== worktree.base_branch) {
    actions.push({
      id: "merge",
      label: "Merge",
      description: `Commit the changes and merge ${worktree.branch} into ${worktree.base_branch || "the base branch"}`,
    });
  }
  actions.push({
    id: "delete",
    label: "Delete",
    description: worktree.branch
      ? `Permanently delete the worktree and branch ${worktree.branch}`
      : "Permanently delete the detached worktree",
    danger: true,
  });
  return actions;
}

/** The decisive outline shown after choosing a worktree finish action. */
export function worktreeFinishConfirm(action, worktree) {
  const dirtyFiles = worktree.dirty_files ?? 0;
  const unpushed = worktree.unpushed ?? 0;
  const branch = worktree.branch || null;

  if (action === "cleanup") {
    const actions = ["Remove the clean worktree", branch ? `Keep branch ${branch}` : "No branch will be deleted"];
    if (unpushed) {
      actions.push(
        branch
          ? `Keep ${plural(unpushed, "unpushed commit")} on branch ${branch}`
          : `${plural(unpushed, "unpushed commit")} are not on a branch`,
      );
    }
    return {
      title: "Clean up this worktree?",
      actions,
      confirmLabel: "Clean up",
      danger: false,
    };
  }

  if (action === "push") {
    return {
      title: `Push and finish ${branchLabel(worktree)}?`,
      actions: [
        `Commit ${plural(dirtyFiles, "uncommitted file")}`,
        `Push ${branchLabel(worktree)} to ${upstreamLabel(worktree)}`,
        "Remove the worktree",
        branch ? `Keep branch ${branch}` : "Keep the detached commit",
      ],
      confirmLabel: "Push & finish",
      danger: false,
    };
  }

  if (action === "merge") {
    return {
      title: `Merge and finish ${branchLabel(worktree)}?`,
      actions: [
        `Commit ${plural(dirtyFiles, "uncommitted file")}`,
        `Merge ${branchLabel(worktree)} into ${worktree.base_branch || "the base branch"}`,
        "Remove the worktree",
      ],
      confirmLabel: "Merge & finish",
      danger: false,
    };
  }

  if (action === "delete") {
    const actions = [
      `Permanently lose ${plural(dirtyFiles, "uncommitted file")}`,
      branch ? "Delete the worktree" : "Delete the detached worktree",
    ];
    if (unpushed) actions.push(`Permanently lose ${plural(unpushed, "unpushed commit")}`);
    if (branch) actions.push(`Delete branch ${branch}`);
    return {
      title: "Permanently delete this worktree?",
      intro: "This cannot be undone.",
      actions,
      confirmLabel: "Delete permanently",
      danger: true,
    };
  }

  throw new Error(`Unknown worktree finish action: ${action}`);
}

/** Existing shared-sheet markup for choosing among multiple dirty actions. */
export function worktreeFinishSheetHtml(worktree, actions = worktreeFinishActions(worktree)) {
  const label = worktree.name || worktree.branch || "detached worktree";
  return `<h3>Finish ${esc(label)}</h3>
    <div class="sub">Choose how to finish this worktree. You will confirm before anything changes.</div>
    <div class="addmenu">${actions
      .map(
        (action) =>
          `<button class="btn${action.danger ? " danger" : ""}" data-finish-action="${esc(action.id)}" type="button">${esc(action.label)} · ${esc(action.description)}</button>`,
      )
      .join("")}</div>
    <div class="row"><button class="btn" id="wtfinish-cancel" type="button">Cancel</button></div>`;
}

/** Open the existing shared sheet and report only an explicit action choice. */
export function openWorktreeFinishSheet(worktree, actions, onChoose) {
  const scrim = document.getElementById("scrim");
  const sheet = document.getElementById("sheet");
  if (!scrim || !sheet) return;
  sheet.innerHTML = worktreeFinishSheetHtml(worktree, actions);
  scrim.classList.add("show");
  sheet.querySelector("#wtfinish-cancel").onclick = () => scrim.classList.remove("show");
  sheet.querySelectorAll("[data-finish-action]").forEach((button) => {
    button.onclick = () => {
      scrim.classList.remove("show");
      onChoose(button.dataset.finishAction);
    };
  });
}

function rowError(button, message) {
  const error = button.closest(".srow")?.querySelector("[data-done-error]");
  if (!error) return;
  error.textContent = message;
  error.hidden = !message;
}

async function invokeMutation(button, { confirmation, method, params, callRpc, confirm, refresh }) {
  if (button.disabled) return;
  button.disabled = true;
  rowError(button, "");
  let succeeded = false;
  try {
    if (!(await confirm(confirmation))) return;
    await callRpc(method, params);
    succeeded = true;
    await refresh();
  } catch (error) {
    rowError(button, error instanceof Error ? error.message : String(error));
  } finally {
    if (!succeeded) button.disabled = false;
  }
}

/** Wire all rendered Done controls without owning row navigation handlers. */
export function wireRailDoneControls(
  rail,
  { plans = [], worktrees = [], callRpc, confirm, refresh, openChooser = openWorktreeFinishSheet },
) {
  rail.querySelectorAll("[data-done-plan]").forEach((button) => {
    button.onclick = (event) => {
      event.stopPropagation();
      const plan = plans.find((candidate) => candidate.plan_id === button.dataset.donePlan);
      if (!plan?.can_archive) return;
      invokeMutation(button, {
        confirmation: planArchiveConfirm(),
        method: "plan.archive",
        params: { plan_id: plan.plan_id },
        callRpc,
        confirm,
        refresh,
      });
    };
  });

  rail.querySelectorAll("[data-done-worktree]").forEach((button) => {
    button.onclick = (event) => {
      event.stopPropagation();
      const worktree = worktrees.find((candidate) => candidate.worktree_id === button.dataset.doneWorktree);
      if (!worktree?.can_finish) return;
      const actions = worktreeFinishActions(worktree);
      const invoke = (action) =>
        invokeMutation(button, {
          confirmation: worktreeFinishConfirm(action, worktree),
          method: "worktree.finish",
          params: { project_id: worktree.project_id, worktree_id: worktree.worktree_id, action },
          callRpc,
          confirm,
          refresh,
        });
      if (actions.length === 1) invoke(actions[0].id);
      else openChooser(worktree, actions, invoke);
    };
  });
}
