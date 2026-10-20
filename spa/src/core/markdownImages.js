// External markdown images load only for this button activation. Nothing is
// remembered across instances or cached repaints. Build attachments have their
// own renderer and never pass through this controller.
import { safeImageSrc } from "./markdownWebLinks.js";

const HOST = ":is(.markdown, .thread-body, .task-comment-body, .plan, .thread-event-detail, .cc-body)";

function activatedImage(button, src) {
  const image = button.ownerDocument.createElement("img");
  image.setAttribute("alt", button.dataset.mdImageAlt || "");
  image.setAttribute("loading", "lazy");
  image.setAttribute("referrerpolicy", "no-referrer");
  if (button.dataset.mdImageTitle) image.setAttribute("title", button.dataset.mdImageTitle);
  // Set the network-bearing attribute last, after privacy and text attrs.
  image.setAttribute("src", src);
  return image;
}

/** Native button clicks include keyboard Enter/Space activation. Delegation
 * supports cached repaints, and cleanup removes the sole listener. */
export function installMarkdownImages(root = document) {
  const loaded = new WeakSet();
  const click = (event) => {
    const button = event.target?.closest?.("button.md-image-load");
    if (!button || !root.contains(button) || !button.closest(HOST) || loaded.has(button)) return;
    const src = safeImageSrc(button.dataset.mdImageSrc);
    if (!src) return;
    loaded.add(button);
    button.replaceWith(activatedImage(button, src));
  };
  root.addEventListener("click", click);
  return () => root.removeEventListener("click", click);
}
