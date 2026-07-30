// Pure decisions for the destructive run/plan actions and merge-failure display.
// Kept side-effect-free so the views just render what these return, and the
// bridge-contract rules (which states accept run.abandon / run.delete / plan.*,
// which plans may be Implemented, and the `merge_failed:` error prefix) are
// unit-testable in isolation.

// Terminal *display* states the bridge's run.delete accepts (merged/abandoned/
// archived/failed). A failed run is recoverable by replying, but it can also be
// cleared away, so Delete is the removal affordance we show for it.
const RUN_DELETABLE = new Set(["merged", "abandoned", "archived", "failed"]);

/** Whether run.delete is valid for this state (a terminal display state). */
export function canDelete(state) {
  return RUN_DELETABLE.has(state);
}

/** Whether run.abandon is valid: any live (non-deletable) state. Abandon is the
 *  removal affordance for runs the bridge won't let you delete yet. */
export function canAbandon(state) {
  return !!state && !RUN_DELETABLE.has(state);
}

/** Whether plan.delete is valid: only an abandoned (terminal) plan can be
 *  deleted — a live or approved plan must be abandoned first. */
export function planDeletable(state) {
  return state === "abandoned";
}

/** Whether the plan's first stage doc is approved — the fine-grained gate that
 *  run dispatch requires on top of the coarse plan approval. Single-doc plans
 *  (and migrated legacy single-plan tasks) carry an EMPTY stages array; the
 *  bridge only gates the first stage doc when one exists (orchestrator.rs
 *  dispatch_run: `if let Some(first_stage) = plan.stages.first()`), so an empty
 *  manifest imposes no first-stage gate at all. */
function firstStageApproved(plan) {
  const first = plan && (plan.stages || [])[0];
  if (!first) return true;
  return first.state === "approved";
}

/** Whether Implement (run.create for this plan) is available. The bridge rejects
 *  dispatch unless the plan is approved, its first stage doc is approved, and no
 *  run already implements it (single active writer) — mirror that here so the
 *  action shows as enabled only when it will succeed. */
export function canImplement(plan) {
  if (!plan || plan.state !== "approved") return false;
  if (plan.docs_available === false) return false;
  if (plan.active_run_id) return false;
  return firstStageApproved(plan);
}

/** Why Implement is unavailable, as human copy for a disabled action — or null
 *  when it is available. Ordered by the bridge's own rejection precedence. A plan
 *  whose canonical docs are gone (docs_available false: a migrated plan predating
 *  canonical storage, its worktree pruned) can never be materialized into a run,
 *  so that block precedes the state gates. */
export function implementBlockReason(plan) {
  if (!plan) return "No plan.";
  if (plan.docs_available === false) return "This plan's documents are unavailable, so it can't be implemented.";
  if (plan.active_run_id) return "An implementation is already active for this Issue.";
  if (plan.state !== "approved") return "Approve the plan before implementing it.";
  if (!firstStageApproved(plan)) return "Approve the first stage before implementing.";
  return null;
}

/** Whether a plan doc (the single plan.doc, or a stage's plan.stage_doc) is worth
 *  fetching on this poll. Docs that predate canonical storage (docsAvailable
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

/** Which tab a run card opens on. A run parked between stages (stage_gate) opens
 *  on Stages, where the sequential gate's Start control lives; every other state
 *  — building, review (needs-you), the parked arms — opens on Changes, the diff
 *  review surface that is the product. Pure so notifications/sidebar/project
 *  all pick the same default. */
export function defaultRunTab(run) {
  return run && run.state === "stage_gate" ? "stages" : "changes";
}

/** Where the plan cockpit's back chevron returns to. When the user entered the
 *  plan FROM a run (a sessionStorage marker records that run's id) and that run
 *  is still the plan's active run, the chevron returns to that run's Stages tab —
 *  the plan↔run round trip. A stale marker (run no longer active, or gone) falls
 *  back to the owning project, and a project-less plan to notifications. Pure so
 *  the routing is unit-testable; the view owns the sessionStorage read/removal. */
export function planBackTarget({ returnRunId, activeRunId, projectId }) {
  if (returnRunId && returnRunId === activeRunId)
    return { name: "task", projectId, id: returnRunId, tab: "stages" };
  if (projectId) return { name: "project", projectId };
  return { name: "notifications" };
}

/** Which RPC revises a stage's open comments, given the plan's lifecycle, for
 *  the plan cockpit's per-stage send-notes action. While the plan is still under
 *  review the plan owns the drafting session, so `plan.stage_send_notes` applies.
 *  Once the plan is approved its docs are frozen and the bridge REJECTS
 *  `plan.stage_send_notes`; a live run (active_run_id) then owns the mid-run
 *  revision session, so the notes must go through `run.stage_send_notes`. An
 *  approved plan with no run has no session to revise through — null (disabled).
 *  Returns { method, entityId } — the caller pairs entityId with the method's id
 *  key (plan_id / run_id) and adds stage_id. Pure so both the routing and the
 *  disabled/enabled hint are unit-testable. */
export function stageNotesTarget(plan) {
  if (!plan) return null;
  if (plan.state === "approved") {
    return plan.active_run_id ? { method: "run.stage_send_notes", entityId: plan.active_run_id } : null;
  }
  return { method: "issue.stage_revise", entityId: plan.issue_id || plan.plan_id };
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

/** The confirmation plan for run.delete (removes the terminal run's record). */
export function deleteRunConfirm() {
  return {
    title: "Delete this task?",
    actions: ["Remove the task record permanently"],
    confirmLabel: "Delete",
    danger: true,
  };
}

/** The confirmation plan for plan.delete (removes the abandoned plan's record). */
export function deletePlanConfirm() {
  return {
    title: "Delete this issue?",
    actions: ["Remove the Issue record and stage plans permanently"],
    confirmLabel: "Delete",
    danger: true,
  };
}

/** The confirmation plan for plan.approve — a decisive gate, not a destructive
 *  one: the outline frames what approval unlocks. */
export function approvePlanConfirm() {
  return {
    title: "Mark this issue ready?",
    actions: [
      "The planning worktree is removed — the Issue stage plans are already saved",
      "Implementation unlocks for approved stage plans",
    ],
    confirmLabel: "Mark ready",
    danger: false,
  };
}

/** The confirmation plan for run.create (Implement): what-happens-next framing.
 *  `base` may be a placeholder like "the base branch". */
export function implementConfirm({ base }) {
  return {
    title: "Implement this issue?",
    intro: "A fresh agent session will execute the approved stage plans.",
    actions: [
      `Create a worktree on a new branch off ${base}`,
      "Start a coding agent session for the approved stage plans",
      "You'll be notified when it's ready to review",
    ],
    confirmLabel: "Implement",
    danger: false,
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

/** What the run/plan error banner should show. A locally-held RPC failure (a
 *  failed abandon / delete, which the bridge does NOT record in last_error)
 *  takes precedence over the polled last_error, so the poll can't wipe it before
 *  the user reads it. Falsy for both means the banner is hidden. */
export function bannerText(localError, polledLastError) {
  return localError || polledLastError || "";
}
