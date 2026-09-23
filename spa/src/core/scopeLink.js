// The way out of a narrowed view to the project's whole one (#117): from one
// workspace's chat overview to every workspace's, and from a workspace's Issues
// tab to the project's. One look for both, so the two read as one idea — the
// words say where it goes and the arrow says it goes out.
//
// A link when the way out is a route, a button when it is local state.

import { ICON_SCOPE_OUT } from "./icons.js";
import { esc } from "./text.js";

export function scopeLinkHtml({ label, href = null, className = "" }) {
  const classes = ["scope-link", className].filter(Boolean).join(" ");
  const inner = `<span>${esc(label)}</span>${ICON_SCOPE_OUT}`;
  return href
    ? `<a class="${classes}" href="${esc(href)}">${inner}</a>`
    : `<button type="button" class="${classes}">${inner}</button>`;
}
