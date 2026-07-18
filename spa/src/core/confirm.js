// Reusable modal confirmation for decisive/destructive verbs (merge, approve,
// implement…). The modal outlines the specific actions that will be taken as an
// ordered list, then asks Confirm/Cancel. Pure markup (unit-tested) + a thin
// DOM promise wrapper. Every user-supplied string is escaped.

import { esc } from "./text.js";

const CONFIRM_SCRIM_ID = "confirm-scrim";

/** Pure markup for the confirmation dialog. `actions` are the concrete steps
 *  that will happen on confirm, rendered as an ordered list. All strings
 *  escaped. */
export function confirmModalHtml({
  title,
  intro = "",
  actions = [],
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger = false,
}) {
  const introHtml = intro ? `<div class="sub">${esc(intro)}</div>` : "";
  const stepsHtml = actions.length
    ? `<ol class="confirm-steps">${actions.map((action) => `<li>${esc(action)}</li>`).join("")}</ol>`
    : "";
  const okClass = danger ? "btn primary danger" : "btn primary";
  return (
    `<div class="modal" role="dialog" aria-modal="true">` +
    `<h3>${esc(title)}</h3>` +
    introHtml +
    stepsHtml +
    `<div class="row">` +
    `<button class="btn" data-confirm-cancel>${esc(cancelLabel)}</button>` +
    `<button class="${okClass}" data-confirm-ok>${esc(confirmLabel)}</button>` +
    `</div></div>`
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
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim";
    scrim.id = CONFIRM_SCRIM_ID;
    scrim.innerHTML = confirmModalHtml(opts);
    document.body.appendChild(scrim);

    const settle = (confirmed) => {
      document.removeEventListener("keydown", onKeydown, { capture: true });
      scrim.remove();
      resolve(confirmed);
    };
    const onKeydown = (event) => {
      if (event.key === "Escape") {
        // Capture-phase + stopPropagation so an underlying sheet's Escape
        // handler never sees this press.
        event.stopPropagation();
        settle(false);
      }
    };
    document.addEventListener("keydown", onKeydown, { capture: true });

    scrim.querySelector("[data-confirm-ok]").onclick = () => settle(true);
    scrim.querySelector("[data-confirm-cancel]").onclick = () => settle(false);
    scrim.onclick = (event) => {
      if (event.target === scrim) settle(false);
    };
    scrim.querySelector("[data-confirm-cancel]").focus();
  });
}
