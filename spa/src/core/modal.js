import { hide, reveal, settleHidden } from "./motion.js";

export function modalDialogHtml(innerHtml, { className = "" } = {}) {
  return `<div class="modal${className ? ` ${className}` : ""}" role="dialog" aria-modal="true">${innerHtml}</div>`;
}

export function openModal({ dialogHtml, scrimId = "", onClose = null, canDismiss = () => true, host = document.body }) {
  const scrim = document.createElement("div");
  scrim.className = host === document.body ? "modal-scrim" : "modal-scrim modal-scrim-local";
  if (scrimId) scrim.id = scrimId;
  scrim.innerHTML = dialogHtml;
  const dialog = scrim.firstElementChild;
  scrim.hidden = true;
  dialog.hidden = true;
  host.appendChild(scrim);

  const onScreen = Promise.all([reveal(scrim, { axis: "opacity" }), reveal(dialog, { axis: "height" })]);

  let closing = null;
  let visible = true;
  let interrupted = false;
  const close = () => {
    if (closing) return closing;
    document.removeEventListener("keydown", onKeydown, { capture: true });
    closing = Promise.all([hide(dialog, { axis: "height" }), hide(scrim, { axis: "opacity" })]).then(() => {
      scrim.remove();
      if (onClose) onClose();
    });
    return closing;
  };
  const onKeydown = (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    if (canDismiss()) close();
  };
  document.addEventListener("keydown", onKeydown, { capture: true });
  scrim.onclick = (event) => {
    if (event.target === scrim && canDismiss()) close();
  };

  onScreen.then(() => {
    if (closing || !visible || interrupted || scrim.contains(document.activeElement)) return;
    dialog.querySelector("button")?.focus();
  });

  // Retained surfaces detach their modal synchronously. Countermand any reveal
  // first, so a queued animation cannot put it back or move focus after hide.
  const setVisible = (shown) => {
    if ((closing && shown) || shown === visible) return;
    visible = shown;
    interrupted = true;
    if (!shown) {
      if (scrim.contains(document.activeElement)) document.activeElement.blur();
      document.removeEventListener("keydown", onKeydown, { capture: true });
      settleHidden(dialog);
      settleHidden(scrim);
      scrim.remove();
    } else {
      host.appendChild(scrim);
      scrim.hidden = false;
      dialog.hidden = false;
      document.addEventListener("keydown", onKeydown, { capture: true });
    }
  };
  return { body: dialog, close, setVisible };
}
