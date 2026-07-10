// Minimal, safe markdown → HTML for plans and notification messages.
// Everything is HTML-escaped; only the small vocabulary below is rendered.

import { esc } from "./text.js";
import { slugifyHeading } from "./anchors.js";

export function renderMarkdown(markdown) {
  const lines = (markdown || "").split("\n");
  let html = "";
  let inCode = false;
  let inList = false;
  const inline = (s) =>
    esc(s)
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  // Heading ids are computed from the raw heading text (before inline rendering)
  // and de-duplicated within a single render so the stages view can scroll a
  // comment's breadcrumb to `#<slug>`. Empty slug → omit the id attribute.
  const headingCounts = new Map();
  const idAttr = (raw) => {
    const base = slugifyHeading(raw);
    if (!base) return "";
    const n = (headingCounts.get(base) || 0) + 1;
    headingCounts.set(base, n);
    return ` id="${n === 1 ? base : `${base}-${n}`}"`;
  };
  for (const line of lines) {
    if (line.startsWith("```")) {
      inCode = !inCode;
      html += inCode ? "<pre><code>" : "</code></pre>";
      continue;
    }
    if (inCode) {
      html += esc(line) + "\n";
      continue;
    }
    const listItem = line.match(/^\s*[-*]\s+(.*)/) || line.match(/^\s*\d+\.\s+(.*)/);
    if (listItem) {
      if (!inList) {
        html += "<ul>";
        inList = true;
      }
      html += `<li>${inline(listItem[1])}</li>`;
      continue;
    }
    if (inList) {
      html += "</ul>";
      inList = false;
    }
    if (line.startsWith("### ")) {
      const raw = line.slice(4);
      html += `<h3${idAttr(raw)}>${inline(raw)}</h3>`;
    } else if (line.startsWith("## ")) {
      const raw = line.slice(3);
      html += `<h2${idAttr(raw)}>${inline(raw)}</h2>`;
    } else if (line.startsWith("# ")) {
      const raw = line.slice(2);
      html += `<h1${idAttr(raw)}>${inline(raw)}</h1>`;
    } else if (line.trim()) html += `<p>${inline(line)}</p>`;
  }
  if (inList) html += "</ul>";
  if (inCode) html += "</code></pre>";
  return html;
}
