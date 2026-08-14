// The reviewer's half of the review-prioritization overlay: disagreeing with
// where a triage pass put a hunk.
//
// Both surfaces that draw an ordered stack (the Changes pane and the aggregate
// review plug) mount one of these, because the interaction is the same in both:
// press the offer on the hunk, optionally say why, and the stack re-orders on
// the tap — not on the next poll. The bridge is told after, and the pass it
// answers with is what the surface renders from then on.
//
// Why hold anything locally at all: the pass arrives on a poll, so between the
// tap and the next read the surface would otherwise repaint the reviewer's own
// decision away. A sent override is held here exactly until the pass comes back
// carrying it (core/triageModel unsettledOverrides), so the reviewer's word is
// on screen continuously and is rendered from ONE place — the pass — as soon as
// there is a pass that says it.

import { showNotePop, hideCommentPop } from "../commentPop.js";
import { applyTriageOverrides, unsettledOverrides } from "./triageModel.js";
import { notifyError } from "./notify.js";

/** What the composer's confirm button says, per direction: the verb the press
 *  performs, so the button does the thing rather than "saving" a note. */
const CONFIRM_LABEL = { surface: "Keep surfaced", collapse: "Collapse" };
const PLACEHOLDER = {
  surface: "Why does this need reading? (optional)",
  collapse: "Why does this not need reading? (optional)",
};

/**
 * createTriageOverrides({ post, onChange }) → the override layer for one
 * surface.
 *
 * - `post({ hunk_id, direction, note })` sends `triage.override`; a rejection
 *   un-does the local reading and says so, because a disagreement the bridge
 *   did not record is not a disagreement.
 * - `onChange()` asks the surface to repaint (the optimistic re-order, and the
 *   revert if the RPC fails).
 *
 * `apply(triage)` is what the surface renders: the pass with everything sent
 * and not yet echoed applied over it. `handleClick(event)` claims a press on an
 * override control and answers whether it did.
 */
export function createTriageOverrides({ post, onChange }) {
  let pending = [];

  const record = async (override) => {
    pending = [...pending.filter((sent) => sent.hunk_id !== override.hunk_id), override];
    onChange();
    try {
      await post(override);
    } catch (error) {
      pending = pending.filter((sent) => sent !== override);
      onChange();
      notifyError("Could not record your review-order correction", error.message);
    }
  };

  return {
    apply(triage) {
      if (!triage) return triage;
      pending = unsettledOverrides(triage, pending);
      return applyTriageOverrides(triage, pending);
    },

    handleClick(event) {
      const control = event.target.closest && event.target.closest(".toverride");
      if (!control) return false;
      const { hunk: hunkId, direction } = control.dataset;
      showNotePop(control.getBoundingClientRect(), {
        placeholder: PLACEHOLDER[direction] || "Why? (optional)",
        confirmLabel: CONFIRM_LABEL[direction] || "Save",
        onSubmit: (note) => record({ hunk_id: hunkId, direction, note }),
      });
      return true;
    },

    dispose() {
      hideCommentPop();
    },
  };
}
