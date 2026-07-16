// Pure decisions for the destructive task actions and merge-failure display.
// Kept side-effect-free so the view just renders what these return, and the
// bridge-contract rules (which states accept task.abandon / task.delete, and the
// `merge_failed:` error prefix) are unit-testable in isolation.

// Terminal *display* states the bridge's task.delete accepts (merged/abandoned/
// archived/failed). A failed task is recoverable by replying, but it can also be
// cleared off the board, so Delete is the removal affordance we show for it.
const DELETABLE = new Set(["merged", "abandoned", "archived", "failed"]);

/** Whether task.delete is valid for this state (terminal on the board). */
export function canDelete(state) {
  return DELETABLE.has(state);
}

/** Whether task.abandon is valid: any live (non-deletable) state. Abandon is the
 *  removal affordance for tasks the bridge won't let you delete yet. */
export function canAbandon(state) {
  return !!state && !DELETABLE.has(state);
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

/** What the task error banner should show. A locally-held RPC failure (a failed
 *  task.abandon / task.delete, which the bridge does NOT record in last_error)
 *  takes precedence over the polled last_error, so the 1.6s poll can't wipe it
 *  before the user reads it. Falsy for both means the banner is hidden. */
export function bannerText(localError, polledLastError) {
  return localError || polledLastError || "";
}
