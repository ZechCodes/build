// The box you talk to an agent from.
//
// A conversation about code is mostly words, but the moments that need help are
// the ones words are worst at: a screenshot of the misaligned button, the
// stack trace that is four hundred lines long, the mock somebody sent you. So
// the composer takes files — picked, pasted, or dropped — and a paste too big
// to read becomes one instead of burying the box it was pasted into.
//
// The bytes go up BEFORE the message does (`thread.attach`), so a send is
// always just a message naming files that already exist. That is what lets a
// failed upload sit on its own chip instead of failing the whole send, and what
// lets the tray survive the timeline repainting under it: the tray's state is
// the view's draft, not the DOM's.

import { esc } from "./text.js";
import { ICON_ARROW_RIGHT, ICON_PAPERCLIP, ICON_X } from "./icons.js";
import { splitButtonMarkup } from "./splitButton.js";

/// Mirrors the bridge's own cap (`ATTACHMENT_MAX_BYTES`). Checked here too, so
/// a file that cannot land is refused before it is read rather than after a
/// slow encode and a round trip.
export const ATTACHMENT_MAX_BYTES = 5 * 1024 * 1024;

/// Past this many characters a paste stops being text you are writing and
/// starts being a document you are handing over — a log, a trace, a file's
/// contents. Below it, the browser's own paste is exactly right.
export const LARGE_PASTE_CHARS = 1200;

/// How many files one message may carry (the bridge enforces the same).
export const ATTACHMENTS_PER_MESSAGE_MAX = 10;

export function formatAttachmentSize(bytes) {
  const size = Number(bytes) || 0;
  if (size < 1000) return `${Math.round(size)} B`;
  if (size < 1000 * 1000) return `${Math.round(size / 1000)} KB`;
  return `${(size / (1000 * 1000)).toFixed(1)} MB`;
}

export function isImageAttachment(mime) {
  return String(mime || "").startsWith("image/");
}

/// What a paste is asking for.
///
/// Files win over text whenever the clipboard holds any: a screenshot copied
/// out of a viewer carries its own filename as text too, and pasting that
/// filename is never what was meant. Some sources expose the image only as a
/// clipboard ITEM rather than a file, so both are read.
export function pasteIntent(clipboardData) {
  if (!clipboardData) return { files: [], asFile: null };
  const files = [...(clipboardData.files || [])];
  for (const item of clipboardData.items || []) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile && item.getAsFile();
    if (file && !files.includes(file)) files.push(file);
  }
  if (files.length) return { files, asFile: null };
  const text = clipboardData.getData ? clipboardData.getData("text/plain") || "" : "";
  return { files: [], asFile: text.length > LARGE_PASTE_CHARS ? text : null };
}

/// A pasted wall of text, named so the reviewer can tell two of them apart.
export function largePasteFile(text, ordinal) {
  return new File([text], `pasted-text-${ordinal}.txt`, { type: "text/plain" });
}

function readAsDataUrl(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(reader.error || new Error(`could not read ${file.name}`));
    reader.readAsDataURL(file);
  });
}

/// The base64 payload of a `data:` URL — what the bridge wants, without a
/// second pass over the bytes.
export function base64OfDataUrl(dataUrl) {
  const comma = String(dataUrl || "").indexOf(",");
  return comma === -1 ? "" : dataUrl.slice(comma + 1);
}

// ---- markup ---------------------------------------------------------------

const chipHtml = (entry, index) => {
  const thumb = isImageAttachment(entry.mime) && entry.dataUrl
    ? `<img class="composer-chip-thumb" src="${esc(entry.dataUrl)}" alt="">`
    : `<span class="composer-chip-glyph" aria-hidden="true">${esc(extensionLabel(entry.name))}</span>`;
  const note = entry.status === "uploading"
    ? "Attaching…"
    : entry.status === "failed"
      ? entry.error || "Could not attach"
      : formatAttachmentSize(entry.size);
  return `<div class="composer-chip ${esc(entry.status)}" data-index="${index}">
    ${thumb}
    <span class="composer-chip-meta">
      <span class="composer-chip-name" title="${esc(entry.name)}">${esc(entry.name)}</span>
      <span class="composer-chip-note">${esc(note)}</span>
    </span>
    <button type="button" class="composer-chip-remove" data-index="${index}" aria-label="Remove ${esc(entry.name)}">${ICON_X}</button>
  </div>`;
};

/// The three or four characters that say what a file is when there is no
/// thumbnail to show — its extension, or a generic mark.
function extensionLabel(name) {
  const dot = String(name || "").lastIndexOf(".");
  const extension = dot > 0 ? name.slice(dot + 1) : "";
  return extension && extension.length <= 4 ? extension.toUpperCase() : "FILE";
}

/// Ids for the parts a caller never names itself, derived from the input's own
/// id so two composers on one page cannot collide.
export const composerPartIds = (inputId) => ({
  tray: `${inputId}tray`,
  attach: `${inputId}attach`,
  file: `${inputId}file`,
  sendControl: `${inputId}sendcontrol`,
});

/// The two ways one message can reach an agent that is already working.
///
/// The default is the send it has always been: the message is queued and the
/// agent takes it at the next step of the turn it is running — which, for a
/// carrier that can be steered mid-turn, usually decides that turn's outcome.
/// The alternative stops the turn first. It is never the default press: the
/// queued send costs nothing and mostly gets there anyway, so the human reaches
/// for the interrupt deliberately or not at all.
export const SEND_OPTION = {
  id: "send",
  label: "Send",
  description: "Hand this message to the agent at its next step",
  busyLabel: "sending…",
};
export const INTERRUPT_SEND_OPTION = {
  id: "interrupt_send",
  menuLabel: "Interrupt & send",
  description: "Stop what the agent is doing now and hand it this message",
  busyLabel: "sending…",
};

/// The send control in its two shapes, keyed by whether there is a turn to
/// stop. The button is named `sendId` in both, so one lookup wires either.
export function sendControlHtml({ sendId, canInterrupt = false }) {
  if (!canInterrupt) {
    return `<button class="btn primary composer-send" id="${esc(sendId)}"><span class="composer-send-label">Send</span>${ICON_ARROW_RIGHT}</button>`;
  }
  return splitButtonMarkup([SEND_OPTION, INTERRUPT_SEND_OPTION], { variant: "primary", primaryId: sendId });
}

/// The composer's markup. `attachable` adds the paperclip and the tray; a
/// surface with no upload path renders the plain box. `canInterrupt` is what
/// the send control is showing right now — a poll moves it in place rather
/// than rebuilding the box around it.
export function composerHtml({ inputId, sendId, hintId, placeholder, attachable = false, canInterrupt = false }) {
  const parts = composerPartIds(inputId);
  const attachControls = attachable
    ? `<input type="file" id="${esc(parts.file)}" class="composer-file" multiple hidden>
       <button type="button" class="composer-attach" id="${esc(parts.attach)}" aria-label="Attach files" title="Attach files">${ICON_PAPERCLIP}</button>`
    : "";
  return `<div class="thread-composer">
    <div class="composer${attachable ? " attachable" : ""}">
      ${attachable ? `<div class="composer-tray" id="${esc(parts.tray)}" hidden></div>` : ""}
      <textarea id="${esc(inputId)}" rows="1" placeholder="${esc(placeholder)}"></textarea>
      <div class="composer-bar">
        <span class="hint" id="${esc(hintId)}"></span>
        <span class="composer-shortcut" aria-hidden="true">⌘↵</span>
        <div class="composer-actions">
          ${attachControls}
          <div class="composer-send-control" id="${esc(parts.sendControl)}">${sendControlHtml({ sendId, canInterrupt })}</div>
        </div>
      </div>
      ${attachable ? '<div class="composer-dropmask" aria-hidden="true"><span>Drop to attach</span></div>' : ""}
    </div>
  </div>`;
}

// ---- the tray -------------------------------------------------------------

/// Wire the attachment half of a composer: the paperclip, paste, and drop.
///
/// `upload(file, contentBase64)` resolves to the bridge's attachment descriptor
/// (`{name, path, mime, size}`) — the thing a send names. `readAttachments` /
/// `writeAttachments` are the view's draft: without them the tray is emptied by
/// the next repaint, which on a polling surface is about a second away.
///
/// Returns a controller: `attachments()` for the descriptors a send should
/// carry, `busy()` for whether an upload is still in flight, and `clear()` for
/// after a send lands.
export function mountComposerAttachments(root, {
  ids,
  upload,
  onError = () => {},
  onChange = () => {},
  readAttachments = null,
  writeAttachments = null,
}) {
  const parts = composerPartIds(ids.input);
  const composer = root.querySelector(".composer");
  const tray = root.querySelector(`#${parts.tray}`);
  const input = root.querySelector(`#${ids.input}`);
  const picker = root.querySelector(`#${parts.file}`);
  if (!composer || !tray || !input) return null;

  // The entries are the draft: the same array objects survive a repaint, which
  // is what lets an upload started before it settle into the tray after.
  let entries = (readAttachments && readAttachments()) || [];
  const persist = () => {
    if (writeAttachments) writeAttachments(entries);
  };

  const render = () => {
    tray.hidden = entries.length === 0;
    tray.innerHTML = entries.map(chipHtml).join("");
    tray.querySelectorAll(".composer-chip-remove").forEach((button) => {
      button.onclick = () => {
        entries = entries.filter((_, index) => index !== Number(button.dataset.index));
        persist();
        render();
        onChange();
      };
    });
    onChange();
  };

  const settle = (entry, promise) =>
    promise.then(
      (descriptor) => {
        entry.status = "ready";
        entry.descriptor = descriptor;
        entry.size = descriptor.size ?? entry.size;
        entry.mime = descriptor.mime || entry.mime;
        render();
      },
      (error) => {
        entry.status = "failed";
        entry.error = (error && error.message) || "Could not attach";
        render();
        onError(`${entry.name}: ${entry.error}`);
      },
    );

  // An upload started under a previous mount is still running; adopt it so the
  // repainted tray stops showing "Attaching…" when it finishes.
  for (const entry of entries) {
    if (entry.status === "uploading" && entry.pending) settle(entry, entry.pending);
  }

  const addFiles = (files) => {
    const room = ATTACHMENTS_PER_MESSAGE_MAX - entries.length;
    const accepted = [...files].filter(Boolean).slice(0, Math.max(0, room));
    if (accepted.length < [...files].length) {
      onError(`A message carries at most ${ATTACHMENTS_PER_MESSAGE_MAX} files.`);
    }
    for (const file of accepted) {
      if (file.size > ATTACHMENT_MAX_BYTES) {
        onError(`${file.name} is ${formatAttachmentSize(file.size)}; the limit is ${formatAttachmentSize(ATTACHMENT_MAX_BYTES)}.`);
        continue;
      }
      const entry = {
        name: file.name,
        size: file.size,
        mime: file.type || "application/octet-stream",
        status: "uploading",
        dataUrl: "",
        descriptor: null,
        error: "",
      };
      entries = [...entries, entry];
      entry.pending = readAsDataUrl(file).then((dataUrl) => {
        // Kept for the thumbnail: the read already cost the bytes, so an image
        // preview is free and no object URL needs revoking.
        if (isImageAttachment(entry.mime)) entry.dataUrl = dataUrl;
        return upload(file, base64OfDataUrl(dataUrl));
      });
      settle(entry, entry.pending);
    }
    persist();
    render();
  };

  if (picker) {
    picker.onchange = () => {
      addFiles(picker.files || []);
      // Re-picking the same file must fire change again.
      picker.value = "";
    };
    const attach = root.querySelector(`#${parts.attach}`);
    if (attach) attach.onclick = () => picker.click();
  }

  input.addEventListener("paste", (event) => {
    const intent = pasteIntent(event.clipboardData);
    if (intent.files.length) {
      event.preventDefault();
      addFiles(intent.files);
      return;
    }
    if (intent.asFile !== null) {
      event.preventDefault();
      const ordinal = entries.filter((entry) => entry.name.startsWith("pasted-text-")).length + 1;
      addFiles([largePasteFile(intent.asFile, ordinal)]);
    }
  });

  // dragenter/dragover must both be cancelled or the browser navigates to the
  // dropped file instead of handing it over.
  const showDrop = (event) => {
    event.preventDefault();
    composer.classList.add("is-dropping");
  };
  root.addEventListener("dragenter", showDrop);
  root.addEventListener("dragover", showDrop);
  root.addEventListener("dragleave", (event) => {
    // Only the drag actually leaving the composer clears it — moving between
    // children fires dragleave constantly.
    if (event.relatedTarget && root.contains(event.relatedTarget)) return;
    composer.classList.remove("is-dropping");
  });
  root.addEventListener("drop", (event) => {
    event.preventDefault();
    composer.classList.remove("is-dropping");
    const transfer = event.dataTransfer;
    if (!transfer) return;
    const files = [...(transfer.files || [])];
    if (files.length) addFiles(files);
  });

  render();

  return {
    /// The descriptors a send should carry: uploaded, in tray order. A failed
    /// or in-flight chip contributes nothing.
    attachments() {
      return entries.filter((entry) => entry.status === "ready").map((entry) => entry.descriptor);
    },
    busy() {
      return entries.some((entry) => entry.status === "uploading");
    },
    /// Whether the tray holds anything at all — including a chip still going
    /// up, which is what makes an empty-bodied send worth waiting for.
    isEmpty() {
      return entries.length === 0;
    },
    clear() {
      entries = [];
      persist();
      render();
    },
    addFiles,
  };
}

/// Grow the box to the text in it. A one-line reply should not sit in a
/// five-line well, and a paragraph should not be read through a slot.
///
/// The CEILING is the stylesheet's — `max-height` on the textarea — so each
/// surface caps its own box: a full-width page can afford a taller one than the
/// agent rail, where a growing box is taking its room from the conversation it
/// is a reply to. Past the ceiling the box scrolls its own text.
export function autoGrow(input) {
  const fit = () => {
    input.style.height = "auto";
    input.style.height = `${input.scrollHeight}px`;
  };
  input.addEventListener("input", fit);
  fit();
  return fit;
}
