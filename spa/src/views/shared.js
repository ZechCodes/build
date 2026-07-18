// Presentation helpers shared by notifications, project, and the run/plan
// views. The pure presentation helpers (labels, chip palettes, payload copy,
// attention filters, terminal-state sets) live in core/entityPresentation.js
// so they are unit-testable in node; this module re-exports them so existing
// importers keep a single import point, and adds setBadge (which needs the DOM
// and App).

import { $ } from "../dom.js";
import { App } from "../app.js";
import {
  RUN_TERMINAL_STATES,
  PLAN_TERMINAL_STATES,
  RUN_STATE_LABEL,
  runChipClass,
  runPayloadFor,
  attnRuns,
  PLAN_STATE_LABEL,
  planChipClass,
  planPayloadFor,
  attnPlans,
} from "../core/entityPresentation.js";

export {
  RUN_TERMINAL_STATES,
  PLAN_TERMINAL_STATES,
  RUN_STATE_LABEL,
  runChipClass,
  runPayloadFor,
  attnRuns,
  PLAN_STATE_LABEL,
  planChipClass,
  planPayloadFor,
  attnPlans,
};

// ---- The attention badge (runs + plans) --------------------------------------

/** The nav badge counts every run *and* plan that needs the user and is not yet
 *  read. Runs are keyed by run_id, plans by plan_id. */
export function setBadge(runs, plans = []) {
  const unread =
    attnRuns(runs).filter((r) => !App.readIds.has(r.run_id)).length +
    attnPlans(plans).filter((p) => !App.readIds.has(p.plan_id)).length;
  const badge = $("#notif");
  if (!badge) return;
  badge.style.display = unread ? "inline-block" : "none";
  badge.textContent = unread;
}
