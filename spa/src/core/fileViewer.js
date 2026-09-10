const RENDERED_MIMES = new Set(["text/markdown", "text/html", "image/svg+xml"]);

function binaryMime(mime = "") {
  if (mime === "application/octet-stream" || mime === "application/pdf") return true;
  return ["image/", "audio/", "video/"].some((prefix) => mime.startsWith(prefix)) && mime !== "image/svg+xml";
}

function sourceAvailable(file, rendered) {
  if (rendered || file.encoding === "utf-8" || file.editable === true) return true;
  return !file.truncated && file.mime?.startsWith("text/");
}

function editAvailable(file) {
  if (file.truncated || file.editable !== true || !file.revision) return false;
  return file.encoding == null || file.encoding === "utf-8";
}

export function fileViewerModes(file) {
  if (!file) return [];
  const rendered = RENDERED_MIMES.has(file.mime);
  if (binaryMime(file.mime)) return [];
  return [...(rendered ? ["preview"] : []), ...(sourceAvailable(file, rendered) ? ["source"] : []), ...(editAvailable(file) ? ["edit"] : [])];
}

export function createFileViewerState({ file, text }) {
  const modes = fileViewerModes(file);
  let mode = modes[0] || "preview";
  let value = text;
  let selection = { start: 0, end: 0 };
  let revision = file.revision;

  return {
    choose(next) {
      if (modes.includes(next)) mode = next;
    },
    edit(next, nextSelection = selection) {
      value = next;
      selection = nextSelection;
    },
    saved(nextFile, savedValue = value) {
      file = nextFile;
      revision = nextFile.revision;
      text = savedValue;
    },
    snapshot() {
      return { mode, modes, value, selection, revision, dirty: value !== text, file };
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
