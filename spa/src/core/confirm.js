// Reusable modal confirmation for decisive/destructive verbs (merge, approve,
// implement…). The modal outlines the specific actions that will be taken as an
// ordered list, then asks Confirm/Cancel. Pure markup (unit-tested) + a thin
// DOM promise wrapper. Every user-supplied string is escaped.

import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";

const CONFIRM_SCRIM_ID = "confirm-scrim";

/** Pure markup for the confirmation dialog. `actions` are the concrete steps
 *  that will happen on confirm, rendered as an ordered list. `warnings` are what
 *  the verb is about to cost — the bridge's own preflight — and they are read
 *  first, because they are the part that changes the answer. All strings
 *  escaped. */
export function confirmModalHtml({
  title,
  intro = "",
  warnings = [],
  actions = [],
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger = false,
}) {
  const introHtml = intro ? `<div class="sub">${esc(intro)}</div>` : "";
  const warningsHtml = warnings.length
    ? `<ul class="confirm-warnings">${warnings.map((warning) => `<li>${esc(warning)}</li>`).join("")}</ul>`
    : "";
  const stepsHtml = actions.length
    ? `<ol class="confirm-steps">${actions.map((action) => `<li>${esc(action)}</li>`).join("")}</ol>`
    : "";
  const okClass = danger ? "btn primary danger" : "btn primary";
  return modalDialogHtml(
    `<h3>${esc(title)}</h3>` +
      introHtml +
      warningsHtml +
      stepsHtml +
      `<div class="row">` +
      `<button class="btn" data-confirm-cancel>${esc(cancelLabel)}</button>` +
      `<button class="${okClass}" data-confirm-ok>${esc(confirmLabel)}</button>` +
      `</div>`,
  );
}

/** Whether a confirm modal is currently on screen (sheet dismiss handlers
 *  check this so Escape closes the modal, not the sheet underneath). */
export function isConfirmOpen() {
  return !!document.getElementById(CONFIRM_SCRIM_ID);
}

/** Show the confirmation modal; resolves true on Confirm, false on Cancel,
 *  scrim click, or Escape. Focus lands on Cancel (safe default for
 *  destructive verbs); Enter confirms only when focus is on the ok button. */
export function confirmAction(opts) {
  return new Promise((resolve) => {
    // The modal's own ways out — Escape, the scrim — are a cancel, so the
    // answer starts at false and only a press on Confirm moves it.
    let answer = false;
    const { body, close } = openModal({
      dialogHtml: confirmModalHtml(opts),
      scrimId: CONFIRM_SCRIM_ID,
      onClose: () => resolve(answer),
    });
    const settle = (confirmed) => {
      answer = confirmed;
      close();
    };
    body.querySelector("[data-confirm-ok]").onclick = () => settle(true);
    const cancel = body.querySelector("[data-confirm-cancel]");
    cancel.onclick = () => settle(false);
    cancel.focus();
  });
}
