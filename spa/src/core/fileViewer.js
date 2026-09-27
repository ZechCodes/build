const RENDERED_MIMES = new Set(["text/markdown", "text/html", "image/svg+xml"]);
const MEDIA_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "mp3", "wav", "m4a", "aac", "flac", "mp4", "m4v", "webm", "mov"]);

/** Mirrors the bridge's extension MIME table for choosing a raw first page. */
export const isMediaPath = (path) => MEDIA_EXTENSIONS.has(path.split(".").at(-1)?.toLowerCase());

const SIZE_ONLY_MIMES = new Set(["application/octet-stream", "application/pdf"]);

const mediaMime = (mime) =>
  ["image/", "audio/", "video/"].some((prefix) => (mime || "").startsWith(prefix)) && mime !== "image/svg+xml";

function binaryMime(mime = "") {
  return SIZE_ONLY_MIMES.has(mime) || mediaMime(mime);
}

/**
 * How much of a file the viewer needs, which is how much of a file too large
 * for one cache record is read into pages (#95):
 *
 * - `none`: a binary the viewer shows as its size alone, so no bytes at all;
 * - `media`: an image, a sound or a film, shown whole from every byte and
 *   never as source;
 * - `rendered`: an HTML page or an SVG, shown whole too, with a source view;
 * - `lines`: everything else, shown as source a page at a time. Markdown is
 *   among them: a part of a document cannot be rendered as one.
 */
export function fileBodyReading(mime = "") {
  if (SIZE_ONLY_MIMES.has(mime)) return "none";
  if (mediaMime(mime)) return "media";
  return mime === "text/html" || mime === "image/svg+xml" ? "rendered" : "lines";
}

/** A rendered view needs the whole of a file, which a paged one read as lines
 *  never has in hand: its markdown is shown as source. */
const renderedView = (file) =>
  RENDERED_MIMES.has(file.mime) && !(file.paged && fileBodyReading(file.mime) === "lines");

function sourceAvailable(file, rendered) {
  if (rendered || file.encoding === "utf-8" || file.editable === true) return true;
  return !file.truncated && file.mime?.startsWith("text/");
}

function editAvailable(file) {
  if (file.truncated || file.paged || file.editable !== true || !file.revision) return false;
  return file.encoding == null || file.encoding === "utf-8";
}

export function fileViewerModes(file) {
  if (!file) return [];
  const rendered = renderedView(file);
  if (binaryMime(file.mime)) return [];
  return [...(rendered ? ["preview"] : []), ...(sourceAvailable(file, rendered) ? ["source"] : []), ...(editAvailable(file) ? ["edit"] : [])];
}

const REVISION_CONFLICT = "revision conflict";

/** Whether a record is the very file a baseline was read as. A paged file's
 *  record carries no body and no revision, so the version its pages are of
 *  is what tells two of them apart. */
export const sameFile = (a, b) =>
  a.revision === b.revision && a.content_b64 === b.content_b64 && Boolean(a.truncated) === Boolean(b.truncated)
  && a.of === b.of;

/**
 * One open file's viewer and its draft. Every transition of the draft is here,
 * in one place; the view feeds it the events and paints `snapshot()`.
 *
 * The draft is in one of four states (`snapshot().status`):
 *
 * - `clean`: the buffer is the baseline, and no save is out.
 * - `dirty`: the buffer differs from the baseline, and no save is out.
 * - `saving`: a save is out, and the buffer is what it sent.
 * - `saving-edited`: a save is out, and the buffer has moved on since. That
 *   includes moving back to the old baseline.
 *
 * Beside the state it keeps:
 *
 * - `baseRevision`: the revision the baseline was read at, which the next
 *   save is sent against;
 * - `submittedRevision`: the revision the save that is out was sent against;
 * - `disk`: a record of the file at another revision than the baseline, when
 *   the draft had to keep its edits over it (changed on disk);
 * - `error`: the last save's refusal.
 *
 * The events:
 *
 * - `edit`: the buffer moves, and nothing else does.
 * - `submit`: clean or dirty goes to saving. It answers the write to send,
 *   or null when a save is already out.
 * - `saveSucceeded(file)`: the acknowledged file becomes the baseline, at its
 *   revision and with the text that was sent. It is never the record read back
 *   afterwards, which another writer may already have replaced. The buffer is
 *   kept, so saving goes to clean and saving-edited goes to dirty. A disk note
 *   at the acknowledged revision is spent.
 * - `saveFailed(error)`: back to clean or dirty against the old baseline,
 *   keeping the refusal.
 * - `recordArrived(file)`: a record of the file (a push, a re-read, a resume).
 *   - The baseline's own file answers "same", and spends any disk note.
 *   - Over a draft with edits or a save out it answers "held": the draft
 *     keeps its edits and notes the record as a change on disk.
 *   - Over a clean draft it answers "adopt", and the view reads the record
 *     afresh.
 * - `revert`: the buffer goes back to the baseline, and the draft lets go of
 *   any save that is out and any note or refusal. A save still out lands on
 *   disk and in the record, but no longer here. A closed tab's edits are
 *   discarded this way, once the view has confirmed.
 *
 * A clean draft with a disk note left over (a save acknowledged under a newer
 * record) answers `snapshot().stale`: the view adopts the note.
 */
export function createFileViewerState({ file, text }) {
  const modes = fileViewerModes(file);
  let mode = modes[0] || "preview";
  let value = text;
  let selection = { start: 0, end: 0 };
  let submitted = null; // { value, revision } while a save is out
  let disk = null;
  let error = null;

  const status = () => {
    if (submitted) return value === submitted.value ? "saving" : "saving-edited";
    return value === text ? "clean" : "dirty";
  };

  return {
    choose(next) {
      if (modes.includes(next)) mode = next;
    },
    edit(next, nextSelection = selection) {
      value = next;
      selection = nextSelection;
    },
    revert() {
      value = text;
      submitted = null;
      disk = null;
      error = null;
    },
    submit() {
      if (submitted) return null;
      submitted = { value, revision: file.revision };
      error = null;
      return { ...submitted };
    },
    saveSucceeded(written) {
      if (!submitted) return;
      file = written;
      text = submitted.value;
      submitted = null;
      error = null;
      if (disk && sameFile(disk, written)) disk = null;
    },
    saveFailed(failure) {
      submitted = null;
      error = failure?.message || "Save failed";
    },
    recordArrived(record) {
      if (sameFile(record, file)) {
        disk = null;
        return "same";
      }
      if (status() === "clean") return "adopt";
      disk = record;
      return "held";
    },
    snapshot() {
      const current = status();
      return {
        mode, modes, value, selection, file, disk, error,
        status: current,
        unsaved: current !== "clean",
        stale: current === "clean" && disk ? disk : null,
        conflict: Boolean(error?.includes(REVISION_CONFLICT)),
        baseRevision: file.revision,
        submittedRevision: submitted?.revision ?? null,
      };
    },
  };
}

export const encodeBase64Text = (text) => {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  bytes.forEach((byte) => { binary += String.fromCharCode(byte); });
  return btoa(binary);
};

export function fileModeTrayHtml(modes, active) {
  if (modes.length < 2) return "";
  return `<div class="file-mode-tray" role="tablist" aria-label="File view">${modes.map((mode) =>
    `<button type="button" role="tab" class="file-mode${mode === active ? " active" : ""}" data-file-mode="${mode}" aria-selected="${mode === active}">${mode[0].toUpperCase() + mode.slice(1)}</button>`,
  ).join("")}</div>`;
}
