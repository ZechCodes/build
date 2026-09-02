import { hide, reveal } from "./motion.js";

export function modalDialogHtml(innerHtml, { className = "" } = {}) {
  return `<div class="modal${className ? ` ${className}` : ""}" role="dialog" aria-modal="true">${innerHtml}</div>`;
}

export function openModal({ dialogHtml, scrimId = "", onClose = null, host = document.body }) {
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
    close();
  };
  document.addEventListener("keydown", onKeydown, { capture: true });
  scrim.onclick = (event) => {
    if (event.target === scrim) close();
  };

  onScreen.then(() => {
    if (closing || scrim.contains(document.activeElement)) return;
    dialog.querySelector("button")?.focus();
  });

  return { body: dialog, close };
}
