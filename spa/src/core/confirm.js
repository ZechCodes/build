// Reusable modal confirmation for decisive/destructive verbs (merge, approve,
// implement…). The modal outlines the specific actions that will be taken as an
// ordered list, then asks Confirm/Cancel. Pure markup (unit-tested) + a thin
// DOM promise wrapper. Every user-supplied string is escaped.

import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";

const CONFIRM_SCRIM_ID = "confirm-scrim";
let closeAnchoredConfirm = null;

function renderConfirmContent({ title, intro, warnings, actions, confirmLabel, cancelLabel, danger }) {
  const introHtml = intro ? `<div class="sub">${esc(intro)}</div>` : "";
  const warningsHtml = warnings.length
    ? `<ul class="confirm-warnings">${warnings.map((warning) => `<li>${esc(warning)}</li>`).join("")}</ul>`
    : "";
  const stepsHtml = actions.length
    ? `<ol class="confirm-steps">${actions.map((action) => `<li>${esc(action)}</li>`).join("")}</ol>`
    : "";
  const okClass = danger ? "btn primary danger" : "btn primary";
  return `<h3>${esc(title)}</h3>` + introHtml + warningsHtml + stepsHtml +
    `<div class="row"><button class="btn" data-confirm-cancel>${esc(cancelLabel)}</button>` +
    `<button class="${okClass}" data-confirm-ok>${esc(confirmLabel)}</button></div>`;
}

function confirmContentHtml({
  title,
  intro = "",
  warnings = [],
  actions = [],
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger = false,
} = {}) {
  return renderConfirmContent({ title, intro, warnings, actions, confirmLabel, cancelLabel, danger });
}

/** Pure markup for the confirmation dialog. `actions` are the concrete steps
 *  that will happen on confirm, rendered as an ordered list. `warnings` are what
 *  the verb is about to cost — the bridge's own preflight — and they are read
 *  first, because they are the part that changes the answer. All strings
 *  escaped. */
export function confirmModalHtml(opts) {
  return modalDialogHtml(confirmContentHtml(opts));
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
    let confirmed = false;
    const { body, close } = openModal({
      dialogHtml: confirmModalHtml(opts),
      scrimId: CONFIRM_SCRIM_ID,
      onClose: () => resolve(confirmed),
    });
    const settle = (answer) => {
      confirmed = answer;
      close();
    };
    body.querySelector("[data-confirm-ok]").onclick = () => settle(true);
    const cancel = body.querySelector("[data-confirm-cancel]");
    cancel.onclick = () => settle(false);
    cancel.focus();
  });
}

/** Ask beside the control that initiated the action. The popover stays directly
 * below the control, clamps to the viewport, and cancels if a repaint removes
 * its anchor. */
export function confirmActionAt(anchor, opts) {
  closeAnchoredConfirm?.();
  return new Promise((resolve) => {
    const popover = document.createElement("div");
    popover.className = "confirm-popover";
    popover.setAttribute("role", "dialog");
    popover.setAttribute("aria-modal", "false");
    popover.setAttribute("aria-label", opts.title);
    popover.innerHTML = confirmContentHtml(opts);
    document.body.appendChild(popover);

    const rect = anchor.getBoundingClientRect();
    const gap = 6;
    const edge = 8;
    const width = popover.getBoundingClientRect().width || Math.min(400, window.innerWidth - edge * 2);
    popover.style.top = `${rect.bottom + gap}px`;
    popover.style.left = `${Math.max(edge, Math.min(rect.left, window.innerWidth - width - edge))}px`;
    popover.style.maxHeight = `${Math.max(0, window.innerHeight - rect.bottom - gap - edge)}px`;

    let settled = false;
    const observer = new MutationObserver(() => {
      if (!anchor.isConnected) settle(false);
    });
    const cleanup = () => {
      document.removeEventListener("pointerdown", onPointerDown, { capture: true });
      document.removeEventListener("keydown", onKeydown, { capture: true });
      window.removeEventListener("resize", cancel);
      document.removeEventListener("scroll", onScroll, { capture: true });
      observer.disconnect();
      popover.remove();
      if (closeAnchoredConfirm === cancel) closeAnchoredConfirm = null;
    };
    const settle = (answer) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (anchor.isConnected) anchor.focus();
      resolve(answer);
    };
    const cancel = () => settle(false);
    const onPointerDown = (event) => {
      if (!popover.contains(event.target) && event.target !== anchor) cancel();
    };
    const onKeydown = (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      cancel();
    };
    const onScroll = (event) => {
      if (event.target === document || event.target === window || event.target?.contains?.(anchor)) cancel();
    };
    popover.querySelector("[data-confirm-ok]").onclick = () => settle(true);
    const cancelButton = popover.querySelector("[data-confirm-cancel]");
    cancelButton.onclick = cancel;
    document.addEventListener("pointerdown", onPointerDown, { capture: true });
    document.addEventListener("keydown", onKeydown, { capture: true });
    window.addEventListener("resize", cancel);
    document.addEventListener("scroll", onScroll, { capture: true });
    observer.observe(document.body, { childList: true, subtree: true });
    closeAnchoredConfirm = cancel;
    cancelButton.focus();
  });
}
