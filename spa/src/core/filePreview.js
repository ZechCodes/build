// Pure file preview classification and HTML shared by live Files and reviews.
import { langForPath } from "./highlight.js";
import { sourceRowsHtml } from "./pagedFileView.js";

const exactModes = new Map([
  ["text/markdown", "markdown"], ["text/html", "html"], ["image/svg+xml", "svg"],
  ["application/octet-stream", "binary"], ["application/pdf", "binary"],
]);
const prefixModes = [["image/", "image"], ["audio/", "audio"], ["video/", "video"]];
const wholeOnlyModes = new Set(["html", "svg", "image", "audio", "video"]);

/** The preview mode for a MIME hint and whether the answer was cut. */
export function previewModeFor(mime, truncated) {
  const base = exactModes.get(mime) || prefixModes.find(([prefix]) => (mime || "").startsWith(prefix))?.[1] || "source";
  return truncated && wholeOnlyModes.has(base) ? "toolarge" : base;
}

export const previewHasSourceToggle = (mode) => ["markdown", "html", "svg"].includes(mode);

export function decodeBase64Text(contentB64) {
  const bytes = Uint8Array.from(atob(contentB64 || ""), (character) => character.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

export function sourcePreviewHtml(path, text) {
  return `<div class="fsrc"><table>${sourceRowsHtml(text.split("\n"), langForPath(path))}</table></div>`;
}

export function mediaPreviewHtml(mode) {
  const tag = mode === "audio" ? "audio" : "video";
  const className = mode === "audio" ? "faudio" : "fvideo";
  return `<${tag} class="fmedia ${className}" controls preload="metadata"></${tag}>`;
}
