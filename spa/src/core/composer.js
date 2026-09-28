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
import {
  ICON_ARROW_RIGHT,
  ICON_FILE,
  ICON_FILE_ARCHIVE,
  ICON_FILE_AUDIO,
  ICON_FILE_CODE,
  ICON_FILE_IMAGE,
  ICON_FILE_SPREADSHEET,
  ICON_FILE_TEXT,
  ICON_FILE_VIDEO,
  ICON_PAPERCLIP,
  ICON_SQUARE,
  ICON_X,
} from "./icons.js";
import { menuButtonMarkup, mountSplitMenu } from "./splitButton.js";
import { setMotionRowHtml } from "./motion.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import {
  modelMenuLabel,
  modelMenuSelection,
  modelMenuTitle,
  modelMenuNote,
  modelSelectorOptions,
  reasoningSelectorLabel,
  reasoningSelectorOptions,
} from "./agentChoice.js";
import { applyFieldTraits, fieldTraits } from "./fieldTraits.js";

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
/// clipboard ITEM rather than a file, so both are read — and one image the
/// clipboard offers both ways is still one image. `getAsFile()` hands back a
/// fresh File object each call (Chromium on Linux does), so identity cannot
/// say two are the same; what a file is called, weighs, and is can.
// eslint-disable-next-line complexity -- ratchet: pasteIntent is at 13, cap 10 — reduce it, then drop this line
export function pasteIntent(clipboardData) {
  if (!clipboardData) return { files: [], asFile: null };
  const files = [...(clipboardData.files || [])];
  for (const item of clipboardData.items || []) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile && item.getAsFile();
    if (file && !files.some((held) => sameFile(held, file))) files.push(file);
  }
  if (files.length) return { files, asFile: null };
  const text = clipboardData.getData ? clipboardData.getData("text/plain") || "" : "";
  return { files: [], asFile: text.length > LARGE_PASTE_CHARS ? text : null };
}

/// Whether two File objects describe the same bytes, as far as a paste can
/// tell without reading them.
const sameFile = (a, b) =>
  a === b || (a.name === b.name && a.size === b.size && a.type === b.type && a.lastModified === b.lastModified);

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
    : attachmentGlyphHtml(entry.name, entry.mime, "composer-chip-glyph");
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

/// The extension a name ends in, lower-case, or "" for none.
function extensionOf(name) {
  const dot = String(name || "").lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/// The three or four characters that say what a file is when there is no
/// thumbnail to show — its extension, or a generic mark.
function extensionLabel(name) {
  const extension = extensionOf(name);
  return extension && extension.length <= 4 ? extension.toUpperCase() : "FILE";
}

const CODE_EXTENSIONS = new Set([
  "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rb", "rs", "go", "java", "kt", "swift", "c", "h", "cc", "cpp",
  "hpp", "cs", "php", "sh", "bash", "zsh", "fish", "ps1", "sql", "html", "htm", "css", "scss", "less", "vue",
  "svelte", "lua", "pl", "r", "scala", "ex", "exs", "erl", "hs", "ml", "clj", "dart", "zig", "nim", "toml",
  "yaml", "yml", "json", "jsonl", "xml", "ini", "cfg", "conf", "env", "dockerfile", "makefile", "nix", "tf",
]);
const ARCHIVE_EXTENSIONS = new Set(["zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "whl"]);
const SHEET_EXTENSIONS = new Set(["csv", "tsv", "xls", "xlsx", "ods", "numbers"]);
const TEXT_EXTENSIONS = new Set(["txt", "md", "markdown", "rst", "log", "pdf", "doc", "docx", "rtf", "odt", "tex"]);

/// The kinds a file can be read as, in the order they are tried: a type the
/// browser is sure of first, then the name, because a bridge or a browser
/// often only knows `application/octet-stream`.
const KIND_RULES = [
  ["image", (type) => type.startsWith("image/")],
  ["video", (type) => type.startsWith("video/")],
  ["audio", (type) => type.startsWith("audio/")],
  ["archive", (type, extension) => ARCHIVE_EXTENSIONS.has(extension) || /zip|tar|compressed|archive/.test(type)],
  ["sheet", (type, extension) => SHEET_EXTENSIONS.has(extension) || /spreadsheet|csv|excel/.test(type)],
  ["code", (type, extension) => CODE_EXTENSIONS.has(extension) || /json|xml|javascript|yaml/.test(type)],
  ["text", (type, extension) => TEXT_EXTENSIONS.has(extension) || type.startsWith("text/") || type === "application/pdf"],
];

/// What kind of thing a file is, from its type and its name: one of image,
/// video, audio, archive, sheet, code, text, or file.
export function attachmentKind(name, mime) {
  const type = String(mime || "").toLowerCase();
  const extension = extensionOf(name);
  const match = KIND_RULES.find(([, fits]) => fits(type, extension));
  return match ? match[0] : "file";
}

const KIND_ICON = {
  image: ICON_FILE_IMAGE,
  video: ICON_FILE_VIDEO,
  audio: ICON_FILE_AUDIO,
  archive: ICON_FILE_ARCHIVE,
  sheet: ICON_FILE_SPREADSHEET,
  code: ICON_FILE_CODE,
  text: ICON_FILE_TEXT,
  file: ICON_FILE,
};

/// The tile that stands for a file with no thumbnail: an icon for its kind,
/// with its extension on the tile so `.ts` and `.py` do not wear the same face.
/// Shared by the composer's tray and the conversation's sent files, so a file
/// looks the same before and after it is sent.
export function attachmentGlyphHtml(name, mime, className = "attachment-glyph") {
  const kind = attachmentKind(name, mime);
  const tag = extensionLabel(name);
  return `<span class="${esc(className)} attachment-glyph" data-kind="${esc(kind)}" aria-hidden="true">${KIND_ICON[kind]}${
    tag === "FILE" ? "" : `<span class="attachment-glyph-tag">${esc(tag)}</span>`
  }</span>`;
}

/// Ids for the parts a caller never names itself, derived from the input's own
/// id so two composers on one page cannot collide.
export const composerPartIds = (inputId) => ({
  tray: `${inputId}tray`,
  attach: `${inputId}attach`,
  file: `${inputId}file`,
  sendControl: `${inputId}sendcontrol`,
  modelMenu: `${inputId}model`,
  reasoningMenu: `${inputId}reasoning`,
  context: `${inputId}context`,
  gauge: `${inputId}gauge`,
});

/// The right-hand action is an arrow while there is something to send. When a
/// turn is active and the draft is empty, the same stable button becomes Stop.
export function sendControlHtml({ sendId, canInterrupt = false, hasDraft = false }) {
  const stopping = canInterrupt && !hasDraft;
  const label = stopping ? "Stop agent" : "Send message";
  return `<button type="button" class="btn primary composer-send${stopping ? " is-stop" : ""}" id="${esc(sendId)}" data-action="${stopping ? "stop" : "send"}" aria-label="${label}" title="${label}">${stopping ? ICON_SQUARE : ICON_ARROW_RIGHT}</button>`;
}

/// The composer's markup. `attachable` adds the paperclip and the tray; a
/// surface with no upload path renders the plain box. `modelMenu` opens the
/// slot on the row's left for the model menu (`mountComposerModelMenu` fills
/// it); a surface that passes neither renders the row it always did.
/// A conversation's composer (`modelMenu`) also keeps the room beside the
/// paperclip that its context gauge reads out over; the gauge itself is
/// `composerGaugeHtml`, placed by the caller outside the box (#138).
/// `canInterrupt` is what the send control is showing right now — a poll moves
/// it in place rather than rebuilding the box around it.
export function composerHtml({
  inputId,
  sendId,
  hintId,
  placeholder,
  attachable = false,
  canInterrupt = false,
  modelMenu = false,
}) {
  const parts = composerPartIds(inputId);
  const attachControls = attachable
    ? `<input type="file" id="${esc(parts.file)}" class="composer-file" multiple hidden>
       <button type="button" class="composer-attach" id="${esc(parts.attach)}" aria-label="Attach files" title="Attach files">${ICON_PAPERCLIP}</button>`
    : "";
  return `<div class="thread-composer">
    <div class="composer-context" id="${esc(parts.context)}" hidden></div>
    ${attachable ? `<div class="composer-tray" id="${esc(parts.tray)}" hidden></div>` : ""}
    <div class="composer${attachable ? " attachable" : ""}">
      <textarea id="${esc(inputId)}" rows="1" ${fieldTraits("prose")} placeholder="${esc(placeholder)}"></textarea>
      <div class="composer-bar">
        ${modelMenu ? `<div class="composer-choice-controls">
          <div class="composer-model" id="${esc(parts.modelMenu)}"></div>
          <div class="composer-reasoning" id="${esc(parts.reasoningMenu)}"></div>
        </div>` : ""}
        <span class="hint" id="${esc(hintId)}"></span>
        <div class="composer-actions">
          ${modelMenu ? '<span class="composer-gauge-room" aria-hidden="true"></span>' : ""}
          ${attachControls}
          <div class="composer-send-control" id="${esc(parts.sendControl)}">${sendControlHtml({ sendId, canInterrupt })}</div>
        </div>
      </div>
      ${attachable ? '<div class="composer-dropmask" aria-hidden="true"><span>Drop to attach</span></div>' : ""}
    </div>
  </div>`;
}

/// The context gauge, empty and hidden until `mountContextGauge` writes it.
/// It ticks as the agent works, so it stands outside the box being typed in
/// rather than in the bar: a phone's keyboard lost swipes and taps while the
/// rail rewrote things around the text, and the room the bar keeps for it
/// (`.composer-gauge-room`) is never written. The stylesheet lays it over that
/// room.
export const composerGaugeHtml = (inputId) =>
  `<span class="composer-gauge" id="${esc(composerPartIds(inputId).gauge)}" role="note" hidden></span>`;

// ---- the model menu -------------------------------------------------------

/// What one painting of the menu says, as one string to compare the next
/// against.
const choiceKey = (provider, choice, activeModel, activeEffort) =>
  [provider, choice.model || "", choice.effort || "", activeModel || "", activeEffort || ""].join("/");

/// Wire the menu on the composer's left: what the NEXT turn will run on.
///
/// `onChoose(next)` gets the whole reconciled choice — the caller decides where
/// it goes, which is the bridge for an agent that exists and a draft for one
/// the first send will create. Returns a controller whose `set(catalog,
/// provider, choice)` paints it; a call that would change nothing repaints
/// nothing, because a poll must not shut a menu the human just opened.
export function mountComposerModelMenu(root, { ids, onChoose, cacheKey = null }) {
  const parts = composerPartIds(ids.input);
  const modelSlot = root.querySelector(`#${parts.modelMenu}`);
  const reasoningSlot = root.querySelector(`#${parts.reasoningMenu}`);
  if (!modelSlot || !reasoningSlot) return null;

  // What the menu on screen was painted from: the choice, in words, and the
  // catalog it was read out of — which lands after the first paint and brings
  // the models with it.
  let painted = null;
  let paintedCatalog = null;
  let closeModelMenu = null;
  let closeReasoningMenu = null;
  let openModelMenu = null;
  let openReasoningMenu = null;
  let savedOpen = null;
  let menuRecord = null;
  const applySavedMenu = () => {
    if (savedOpen === "model") {
      closeReasoningMenu?.(false);
      openModelMenu?.(false);
    } else if (savedOpen === "reasoning") {
      closeModelMenu?.(false);
      openReasoningMenu?.(false);
    } else {
      closeModelMenu?.(false);
      closeReasoningMenu?.(false);
    }
  };
  const writeOpen = (kind) => (open) => {
    if (menuRecord) void menuRecord.write({ open: open ? kind : null });
  };

  const render = (catalog, provider, choice, activeModel, activeEffort) => {
    closeModelMenu?.(false);
    closeReasoningMenu?.(false);
    painted = choiceKey(provider, choice, activeModel, activeEffort);
    paintedCatalog = catalog;
    const choose = (action) => {
      const next = modelMenuSelection(action, choice);
      if (menuRecord) {
        savedOpen = null;
        onChoose(next);
        void menuRecord.write({ open: null });
        return;
      }
      render(catalog, provider, next, activeModel, activeEffort);
      onChoose(next);
    };
    modelSlot.innerHTML = menuButtonMarkup(
      modelMenuLabel(catalog, provider, { ...choice, effort: "" }, activeModel),
      modelSelectorOptions(catalog, provider, choice),
      { title: modelMenuTitle(catalog, provider, choice, activeModel), arrow: false, note: modelMenuNote(catalog, provider) },
    );
    ({ closeMenu: closeModelMenu, openMenu: openModelMenu } = mountSplitMenu(modelSlot, {
      onChoose: choose,
      onOpenChange: writeOpen("model"),
    }));
    const reasoningOptions = reasoningSelectorOptions(catalog, provider, choice, activeModel, activeEffort);
    reasoningSlot.hidden = reasoningOptions.length === 0;
    reasoningSlot.innerHTML = reasoningOptions.length
      ? menuButtonMarkup(reasoningSelectorLabel(catalog, provider, choice, activeModel, activeEffort), reasoningOptions, { title: "Reasoning level for the next turn", arrow: false })
      : "";
    const reasoning = reasoningOptions.length
      ? mountSplitMenu(reasoningSlot, { onChoose: choose, onOpenChange: writeOpen("reasoning") })
      : null;
    closeReasoningMenu = reasoning?.closeMenu || null;
    openReasoningMenu = reasoning?.openMenu || null;
    applySavedMenu();
  };

  if (cacheKey) menuRecord = watchUiState(uiAddress({ entityId: cacheKey, view: "composer", kind: "menu", sub: "model" }), (saved) => {
    savedOpen = saved?.open || null;
    applySavedMenu();
  });

  return {
    ready: menuRecord?.ready || Promise.resolve(),
    dispose() {
      menuRecord?.dispose({ flushPending: false });
      closeModelMenu?.(false);
      closeReasoningMenu?.(false);
    },
    set(catalog, provider, choice, activeModel = "", activeEffort = "") {
      if (choiceKey(provider, choice, activeModel, activeEffort) === painted && catalog === paintedCatalog) return;
      render(catalog, provider, choice, activeModel, activeEffort);
    },
  };
}

// ---- the tray -------------------------------------------------------------

/// Wire the attachment half of a composer: the paperclip, paste, and drop.
///
/// `upload(file, contentBase64)` resolves to the bridge's attachment descriptor
/// (`{name, path, mime, size}`) — the thing a send names. `readAttachments` /
/// `writeAttachments` are the view's draft: without them the tray is emptied by
/// the next repaint, which on a polling surface is about a second away.
///
/// `accepting()` says whether paste and drop take files right now. A surface
/// that takes its paperclip off a box it keeps (the task page, when a bridge
/// stops carrying files) answers false until it hangs it back on, and the box
/// takes a paste or a drop the way a plain one would.
///
/// Returns a controller: `attachments()` for the descriptors a send should
/// carry, `busy()` for whether an upload is still in flight, and `clear()` for
/// after a send lands.
// eslint-disable-next-line complexity -- ratchet: mountComposerAttachments is at 15, cap 10 — reduce it, then drop this line
export function mountComposerAttachments(root, {
  ids,
  upload,
  onError = () => {},
  onChange = () => {},
  readAttachments = null,
  writeAttachments = null,
  accepting = () => true,
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
    setMotionRowHtml(tray, entries.map(chipHtml).join(""));
    tray.querySelectorAll(":scope > .composer-chip .composer-chip-remove").forEach((button) => {
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
        persist();
        render();
      },
      (error) => {
        entry.status = "failed";
        entry.error = (error && error.message) || "Could not attach";
        persist();
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
    if (!accepting()) return;
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
    if (!accepting()) return;
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
    if (!accepting()) return;
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
///
/// The box is written only when its height has to change. Collapsing it to
/// measure on every keystroke is two writes on the element being typed into,
/// the kind of churn a phone's keyboard is sensitive to (#138). So the text is
/// measured in a copy of the box instead — same width, same type, off screen —
/// and the box itself hears of it only when the answer moves. The copy sees
/// every edit alike, so text replaced by as much or more that wraps into fewer
/// lines still lets the box shrink.
export function autoGrow(input) {
  const fit = () => {
    const height = measuredHeight(input);
    if (height !== null && input.style.height !== height) input.style.height = height;
  };
  input.addEventListener("input", fit);
  fit();
  return fit;
}

/// What sets where a textarea's text wraps, how tall its lines stand, and the
/// least it stands at.
const MIRRORED_STYLE = [
  "boxSizing", "width", "minHeight", "fontFamily", "fontSize", "fontStyle", "fontWeight", "fontStretch", "fontVariant",
  "fontFeatureSettings", "lineHeight", "letterSpacing", "wordSpacing", "textTransform", "textIndent",
  "whiteSpace", "wordBreak", "overflowWrap", "tabSize", "direction",
  "paddingTop", "paddingRight", "paddingBottom", "paddingLeft",
  "borderTopWidth", "borderRightWidth", "borderBottomWidth", "borderLeftWidth",
];

const mirrors = new WeakMap();

/** A box's copy, hidden and never focused. It carries a box's field traits
 *  like every text field the SPA draws, though no keyboard ever reaches it. */
function newMirror(doc) {
  const mirror = doc.createElement("textarea");
  applyFieldTraits(mirror, "prose");
  mirror.setAttribute("aria-hidden", "true");
  mirror.tabIndex = -1;
  mirror.readOnly = true;
  Object.assign(mirror.style, {
    position: "fixed", top: "0", left: "-10000px", visibility: "hidden", pointerEvents: "none",
    height: "auto", maxHeight: "none", overflow: "hidden", borderStyle: "solid", contain: "layout paint",
  });
  return mirror;
}

/** One copy per document that takes a box's type and text so its height can
 *  be read without touching the box. */
function mirrorFor(doc) {
  let mirror = mirrors.get(doc);
  if (!mirror) {
    mirror = newMirror(doc);
    mirrors.set(doc, mirror);
  }
  if (!mirror.isConnected) doc.body.append(mirror);
  return mirror;
}

/** The height the box's text wants, as the `style.height` that gives it, or
 *  null while the box is not laid out and there is nothing to measure against.
 *  The stylesheet's min- and max-height still bound the box it is written on. */
function measuredHeight(input) {
  const doc = input.ownerDocument;
  const view = doc.defaultView;
  if (!doc.body || !view) return null;
  const style = view.getComputedStyle(input);
  if (style.display === "none" || !style.width || style.width === "auto") return null;
  const mirror = mirrorFor(doc);
  for (const property of MIRRORED_STYLE) {
    if (mirror.style[property] !== style[property]) mirror.style[property] = style[property];
  }
  // Collapsed, the box stands as many rows tall as it asks for; its text can
  // only make it taller.
  if (mirror.rows !== input.rows) mirror.rows = input.rows;
  mirror.value = input.value;
  return `${mirror.scrollHeight}px`;
}
