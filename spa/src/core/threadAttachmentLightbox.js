// The one lightbox: every picture and video an attachment tile opens, in a
// conversation and on a task alike (#116).
//
// It is handed the media of ONE list — a message's files, a task body's, one
// comment's — and the index that was pressed, so the arrows and a swipe move
// through what was said together and nothing else. Each item brings its own
// `source()`, a promise of cached byte pages: a picture already on
// the page answers at once, and a recording that was never fetched is fetched
// when it is shown, with the stage saying so meanwhile.
//
// Esc and a press on the backdrop close it (core/modal.js), and focus goes
// back to the thumbnail of what was showing when it closed — found again by
// path when a repaint replaced the one that was pressed.

import { modalDialogHtml, openModal } from "./modal.js";
import { ICON_CHEVRON_LEFT, ICON_CHEVRON_RIGHT } from "./icons.js";
import { attachMediaSource, releaseMediaSource } from "./mediaBlob.js";

/** How far a finger has to travel sideways, in px, for a swipe to count. */
const SWIPE_MIN_PX = 48;

const stepsHtml = (count) => (count > 1
  ? `<button type="button" class="thread-lightbox-step thread-lightbox-prev" aria-label="Previous attachment">${ICON_CHEVRON_LEFT}</button>
    <button type="button" class="thread-lightbox-step thread-lightbox-next" aria-label="Next attachment">${ICON_CHEVRON_RIGHT}</button>
    <p class="thread-lightbox-count" aria-live="polite"></p>`
  : "");

const lightboxHtml = (count) => modalDialogHtml(`
  <button type="button" class="thread-lightbox-close" aria-label="Close preview">×</button>
  <div class="thread-lightbox-stage"></div>
  ${stepsHtml(count)}
`, { className: "thread-lightbox" });

/** Attach cached bytes only after the complete body has arrived. */
function mediaElement(doc, item, source) {
  const video = item.kind === "video";
  const element = doc.createElement(video ? "video" : "img");
  element.className = video ? "thread-lightbox-video" : "thread-lightbox-image";
  if (video) {
    element.setAttribute("controls", "");
    element.setAttribute("playsinline", "");
    element.setAttribute("preload", "metadata");
    element.setAttribute("aria-label", item.name || "video");
  } else {
    element.setAttribute("alt", item.name || "");
  }
  attachMediaSource(element, source.body || source.pages, source.mime);
  return element;
}

const dialogLabel = (item) => `${item.kind === "video" ? "Video" : "Image preview"}: ${item.name || "attachment"}`;

/** The thumbnail to hand focus back to: the one pressed if it is still on the
 *  page, and otherwise the tile a repaint drew for the same path. */
function thumbnailOf(item) {
  if (item.trigger?.isConnected) return item.trigger;
  const doc = item.trigger?.ownerDocument || document;
  return [...doc.querySelectorAll("button.thread-attachment-preview[data-attachment-path]")]
    .find((tile) => tile.dataset.attachmentPath === item.path) || null;
}

const FOCUSABLE = "button, video[controls]";

/** Tab and Shift+Tab go round the lightbox's own controls and never leave it. */
function trapFocus(dialog, event) {
  if (event.key !== "Tab") return;
  const stops = [...dialog.querySelectorAll(FOCUSABLE)];
  if (!stops.length) return;
  const at = stops.indexOf(dialog.ownerDocument.activeElement);
  const next = event.shiftKey ? (at <= 0 ? stops.length - 1 : at - 1) : (at + 1) % stops.length;
  event.preventDefault();
  stops[next].focus();
}

/** Left and right step, except inside a playing video, whose own controls
 *  take the arrows to seek. */
const stepOfKey = (event) => {
  if (event.target?.tagName === "VIDEO") return 0;
  if (event.key === "ArrowLeft") return -1;
  if (event.key === "ArrowRight") return 1;
  return 0;
};

/** A sideways swipe on the stage steps; a mostly-vertical one is a scroll. */
function wireSwipe(stage, step) {
  let start = null;
  stage.addEventListener("touchstart", (event) => {
    const touch = event.changedTouches?.[0];
    start = touch ? { x: touch.clientX, y: touch.clientY } : null;
  }, { passive: true });
  stage.addEventListener("touchend", (event) => {
    const touch = event.changedTouches?.[0];
    if (!start || !touch) return;
    const dx = touch.clientX - start.x;
    const dy = touch.clientY - start.y;
    start = null;
    if (Math.abs(dx) >= SWIPE_MIN_PX && Math.abs(dx) > Math.abs(dy)) step(dx < 0 ? 1 : -1);
  });
}

/**
 * Open `items[index]`. Each item is `{ trigger, path, kind, name, source }`:
 * `kind` is "image" or "video", and `source()` answers `{body, mime}` or
 * `{pages, mime}`. A shared body lets the tile and lightbox use one Blob.
 */
export function openAttachmentLightbox(items, index = 0) {
  let current = Math.max(0, Math.min(index, items.length - 1));
  let showing = 0; // which show() is the latest, so a slow source cannot land late
  let closed = false;
  const doc = items[current]?.trigger?.ownerDocument || document;
  let onKeydown = () => {};
  let stage;
  const clearStage = () => {
    stage?.querySelectorAll("img,video,audio").forEach(releaseMediaSource);
  };
  const modal = openModal({
    dialogHtml: lightboxHtml(items.length),
    onClose: () => {
      closed = true;
      showing += 1;
      clearStage();
      doc.removeEventListener("keydown", onKeydown);
      thumbnailOf(items[current])?.focus();
    },
  });
  const dialog = modal.body;
  stage = dialog.querySelector(".thread-lightbox-stage");
  const count = dialog.querySelector(".thread-lightbox-count");

  const land = (item, turn, src) => {
    if (closed || turn !== showing) return;
    clearStage();
    stage.replaceChildren(mediaElement(doc, item, src));
  };
  const refuse = (turn) => {
    if (turn === showing) stage.innerHTML = '<p class="thread-lightbox-note">This attachment could not be loaded.</p>';
  };

  function show(at) {
    if (closed) return;
    current = (at + items.length) % items.length;
    const item = items[current];
    const turn = ++showing;
    dialog.setAttribute("aria-label", dialogLabel(item));
    if (count) count.textContent = `${current + 1} of ${items.length}`;
    clearStage();
    stage.innerHTML = '<p class="thread-lightbox-note">Loading…</p>';
    Promise.resolve()
      .then(() => item.source())
      .then((src) => (src ? land(item, turn, src) : refuse(turn)), () => refuse(turn));
  }
  const step = (by) => {
    if (by && items.length > 1) show(current + by);
  };

  dialog.querySelector(".thread-lightbox-close").onclick = modal.close;
  const previous = dialog.querySelector(".thread-lightbox-prev");
  const next = dialog.querySelector(".thread-lightbox-next");
  if (previous) previous.onclick = () => step(-1);
  if (next) next.onclick = () => step(1);
  // On the document rather than the dialog: a press on a thumbnail leaves
  // focus where it was until the dialog has finished opening, and the arrows
  // should work from the first frame.
  onKeydown = (event) => {
    trapFocus(dialog, event);
    const by = stepOfKey(event);
    if (!by) return;
    event.preventDefault();
    step(by);
  };
  doc.addEventListener("keydown", onKeydown);
  wireSwipe(stage, step);
  show(current);
  return { ...modal, show, current: () => current };
}
