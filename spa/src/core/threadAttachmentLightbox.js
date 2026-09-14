import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";

const lightboxHtml = ({ src, alt }) => modalDialogHtml(`
  <button type="button" class="thread-lightbox-close" aria-label="Close image preview">×</button>
  <img class="thread-lightbox-image" src="${esc(src)}" alt="${esc(alt || "")}">
`, { className: "thread-lightbox" });

export function openThreadAttachmentLightbox(trigger, image) {
  const previouslyFocused = trigger?.ownerDocument?.activeElement;
  let removeTrap = () => {};
  const modal = openModal({
    dialogHtml: lightboxHtml(image),
    onClose: () => {
      removeTrap();
      const returnTarget = trigger?.isConnected ? trigger : previouslyFocused;
      if (returnTarget?.isConnected) returnTarget.focus();
    },
  });
  modal.body.setAttribute("aria-label", `Image preview: ${image.alt || "attachment"}`);
  const closeButton = modal.body.querySelector(".thread-lightbox-close");
  closeButton.onclick = modal.close;

  const trapFocus = (event) => {
    if (event.key !== "Tab") return;
    event.preventDefault();
    closeButton.focus();
  };
  modal.body.addEventListener("keydown", trapFocus);
  removeTrap = () => modal.body.removeEventListener("keydown", trapFocus);
  return modal;
}
