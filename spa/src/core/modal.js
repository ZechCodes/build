export function modalDialogHtml(innerHtml, { className = "" } = {}) {
  return `<div class="modal${className ? ` ${className}` : ""}" role="dialog" aria-modal="true">${innerHtml}</div>`;
}

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
