// The tiles a list of attachments is drawn as, wherever one is drawn: under a
// chat message, on a task's body, and under a task's comments (#116).
//
// One markup for all of them, down to the classes and `data-attachment-path`,
// so `wireThreadAttachments` (core/thread.js) fills and opens every one of them
// the same way and the lightbox (core/threadAttachmentLightbox.js) is the only
// one there is.
//
// Three tiles. A picture is a thumbnail of itself. A video is a thumbnail too —
// its first frame once the bytes are here, a play mark over it either way. And
// everything else is a chip that downloads.
//
// Pure: HTML in, no DOM.

import { esc } from "./text.js";
import { attachmentGlyphHtml, formatAttachmentSize, isImageAttachment } from "./composer.js";
import { ICON_PLAY } from "./icons.js";

export const isVideoAttachment = (mime) => String(mime || "").startsWith("video/");

/** Whether a tile opens in the lightbox rather than downloading. */
export const isMediaAttachment = (mime) => isImageAttachment(mime) || isVideoAttachment(mime);

const captionHtml = (name, size) =>
  `<figcaption><span class="thread-attachment-name">${name}</span> <span class="thread-attachment-size">${size}</span></figcaption>`;

const imageTileHtml = ({ path, name, size, state }) => `<figure class="thread-attachment-figure${state}">
    <button type="button" class="thread-attachment-preview" data-attachment-path="${path}" data-attachment-kind="image" aria-label="Open ${name}">
      <img class="thread-attachment-image" data-attachment-path="${path}" alt="${name}">
    </button>
    ${captionHtml(name, size)}
  </figure>`;

/** `muted` and `preload="metadata"` make the frame a picture and nothing
 *  else: the thumbnail never plays, never makes a sound, and asks the element
 *  for no more than the first frame. Playing is the lightbox's. */
const videoTileHtml = ({ path, name, size, state, bytes }) => `<figure class="thread-attachment-figure thread-attachment-video-figure${state}">
    <button type="button" class="thread-attachment-preview" data-attachment-path="${path}" data-attachment-kind="video" aria-label="Play ${name}">
      <video class="thread-attachment-video" data-attachment-path="${path}" data-attachment-size="${bytes}" muted playsinline preload="metadata" tabindex="-1" aria-hidden="true"></video>
      <span class="thread-attachment-play" aria-hidden="true">${ICON_PLAY}</span>
    </button>
    ${captionHtml(name, size)}
  </figure>`;

const chipHtml = ({ attachment, path, name, size }) => `<button type="button" class="thread-attachment" data-attachment-path="${path}" data-attachment-name="${name}" title="Download ${name}">
    ${attachmentGlyphHtml(attachment.name, attachment.mime, "thread-attachment-glyph")}
    <span class="thread-attachment-meta">
      <span class="thread-attachment-name">${name}</span>
      <span class="thread-attachment-size">${size}</span>
    </span>
  </button>`;

const tileFor = (mime) => {
  if (isImageAttachment(mime)) return imageTileHtml;
  if (isVideoAttachment(mime)) return videoTileHtml;
  return chipHtml;
};

/**
 * One attachment's tile. `state` is the class suffix a media tile carries for
 * what its bytes are doing (" unavailable", " waiting"), so a repaint of the
 * same list is the same markup down to the class.
 */
export function attachmentTileHtml(attachment, state = "") {
  return tileFor(attachment.mime)({
    attachment,
    path: esc(attachment.path || ""),
    name: esc(attachment.name || attachment.path || "file"),
    size: esc(formatAttachmentSize(attachment.size)),
    bytes: esc(String(Number(attachment.size) || 0)),
    state,
  });
}

/**
 * A list of them, in one row. `stateOf(attachment)` answers each tile's state
 * suffix; `className` names the surface the row sits on.
 */
export function attachmentListHtml(attachments, { stateOf = () => "", className = "" } = {}) {
  const held = (attachments || []).filter((one) => one && one.path);
  if (!held.length) return "";
  const classes = className ? `thread-attachments ${className}` : "thread-attachments";
  return `<div class="${classes}">${held.map((one) => attachmentTileHtml(one, stateOf(one))).join("")}</div>`;
}
