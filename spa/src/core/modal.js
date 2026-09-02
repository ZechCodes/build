// The one modal on this app's screen: a scrim over everything, one dialog
// centred on it, and the three ways out — Escape, a press on the scrim, or the
// caller closing it itself. The confirm dialog and the surface overlays are
// both this, differing only in the dialog they hand it.

/** Pure markup for the dialog a modal holds. `className` is the variant the
 *  caller's own styles hang off; the role and the modal flag are the same for
 *  every one of them, so they are written here rather than by each caller. */
export function modalDialogHtml(innerHtml, { className = "" } = {}) {
  return `<div class="modal${className ? ` ${className}` : ""}" role="dialog" aria-modal="true">${innerHtml}</div>`;
}

/** Open `dialogHtml` on a scrim and return the dialog element to fill plus the
 *  way to take it back off. `onClose` runs exactly once, whichever of the three
 *  ways out was taken, so a caller can settle a promise or tear down what it
 *  mounted inside without tracking which gesture closed it.
 *
 *  Escape is watched at capture phase and stopped there: a sheet or a drawer
 *  under the modal must never see the press that dismissed the modal. */
export function openModal({ dialogHtml, scrimId = "", onClose = null }) {
  const scrim = document.createElement("div");
  scrim.className = "modal-scrim";
  if (scrimId) scrim.id = scrimId;
  scrim.innerHTML = dialogHtml;
  document.body.appendChild(scrim);

  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKeydown, { capture: true });
    scrim.remove();
    if (onClose) onClose();
  };
  const onKeydown = (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    close();
  };
  document.addEventListener("keydown", onKeydown, { capture: true });
  scrim.onclick = (event) => {
    if (event.target === scrim) close();
  };

  return { body: scrim.firstElementChild, close };
}
