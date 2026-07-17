// Pure decisions for the destructive run/plan actions and merge-failure display.
// Kept side-effect-free so the views just render what these return, and the
// bridge-contract rules (which states accept run.abandon / run.delete / plan.*,
// which plans may be Implemented, and the `merge_failed:` error prefix) are
// unit-testable in isolation.

// Terminal *display* states the bridge's run.delete accepts (merged/abandoned/
// archived/failed). A failed run is recoverable by replying, but it can also be
// cleared off the board, so Delete is the removal affordance we show for it.
const RUN_DELETABLE = new Set(["merged", "abandoned", "archived", "failed"]);

/** Whether run.delete is valid for this state (terminal on the board). */
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

/** Whether plan.abandon is valid: any non-terminal plan (abandoned is the only
 *  terminal plan state). Abandon is the removal affordance for a live plan. */
export function planAbandonable(state) {
  return !!state && state !== "abandoned";
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
  if (plan.active_run_id) return false;
  return firstStageApproved(plan);
}

/** Why Implement is unavailable, as human copy for a disabled action — or null
 *  when it is available. Ordered by the bridge's own rejection precedence. */
export function implementBlockReason(plan) {
  if (!plan) return "No plan.";
  if (plan.active_run_id) return "A run is already implementing this plan.";
  if (plan.state !== "approved") return "Approve the plan before implementing it.";
  if (!firstStageApproved(plan)) return "Approve the first stage before implementing.";
  return null;
}

/** Which tab a run card opens on. A run parked between stages (stage_gate) opens
 *  on Stages, where the sequential gate's Start control lives; every other state
 *  — building, review (needs-you), the parked arms — opens on Changes, the diff
 *  review surface that is the product. Pure so the board/notifications/sidebar
 *  all pick the same default. */
export function defaultRunTab(run) {
  return run && run.state === "stage_gate" ? "stages" : "changes";
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
  return { method: "plan.stage_send_notes", entityId: plan.plan_id };
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
