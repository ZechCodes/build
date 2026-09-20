// Minimal, safe markdown → HTML for plans and notification messages.
// Everything is HTML-escaped; only the small vocabulary below is rendered.

import { esc } from "./text.js";
import { slugifyHeading } from "./anchors.js";

/** One row's cells. The outer pipes are optional (GFM), and `\|` is a literal
 *  pipe inside a cell rather than a boundary — a regex column in a table would
 *  otherwise split into nonsense. Returns null for a line that is no row at
 *  all: a row needs at least one unescaped pipe. */
function tableCells(line) {
  const text = line.trim();
  const cells = [];
  let current = "";
  let sawPipe = false;
  let closedByPipe = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (character === "\\" && text[index + 1] === "|") {
      current += "|";
      index += 1;
      continue;
    }
    if (character !== "|") {
      current += character;
      continue;
    }
    sawPipe = true;
    // A leading pipe opens the row rather than closing an empty first cell.
    if (index === 0) continue;
    cells.push(current);
    current = "";
    closedByPipe = index === text.length - 1;
  }
  if (!closedByPipe) cells.push(current);
  if (!sawPipe) return null;
  return cells.map((cell) => cell.trim());
}

/** The `|---|:--:|` line under a header: every cell is dashes, optionally
 *  colon-anchored. It is what tells a table from a paragraph that happens to
 *  contain pipes, so nothing renders as a table without one. */
function delimiterAlignments(line) {
  const cells = line == null ? null : tableCells(line);
  if (!cells || !cells.length) return null;
  const alignments = [];
  for (const cell of cells) {
    if (!/^:?-+:?$/.test(cell)) return null;
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    alignments.push(left && right ? "center" : right ? "right" : left ? "left" : null);
  }
  return alignments;
}

/**
 * The class every fenced block this renderer emits wears.
 *
 * Stamped here rather than styled through whichever wrapper a view happens to
 * use, because the views do not agree: some wrap the output in `.markdown` and
 * some do not, and a rule hung on that class would fix the chat and miss the
 * issue page's docs. A bare `pre` rule would reach further than markdown — the
 * terminal, the diff view, the revision pane all use one.
 *
 * So the renderer marks its own output (#50). Wherever it is rendered, the
 * block scrolls; nothing else does.
 */
export const CODE_BLOCK_CLASS = "md-code";

// eslint-disable-next-line complexity -- ratchet: renderMarkdown is at 18, cap 10 — reduce it, then drop this line
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
  /** A whole table, header row through its last body row. `start` is the header
   *  line's index and `alignments` the delimiter row's. Returns the HTML and
   *  the index of the last line it consumed, so the caller resumes after it.
   *
   *  Every row is fitted to the header's column count — a short row is padded
   *  and a long one truncated — because a table with ragged rows reads as a
   *  rendering bug rather than as the source's own raggedness. The table scrolls
   *  inside .mdtable: the thread panel is a narrow column, and a wide table has
   *  to give way rather than take the conversation's width with it. */
  const tableFrom = (start, alignments) => {
    const columns = tableCells(lines[start]).slice(0, alignments.length);
    const align = (index) => (alignments[index] ? ` style="text-align:${alignments[index]}"` : "");
    const cellsHtml = (cells, tag) =>
      alignments
        .map((_, index) => `<${tag}${align(index)}>${inline(cells[index] ?? "")}</${tag}>`)
        .join("");
    let html = `<div class="mdtable"><table><thead><tr>${cellsHtml(columns, "th")}</tr></thead>`;
    let body = "";
    let index = start + 2; // the header and its delimiter row
    for (; index < lines.length; index += 1) {
      const cells = tableCells(lines[index]);
      if (!cells) break;
      body += `<tr>${cellsHtml(cells, "td")}</tr>`;
    }
    if (body) html += `<tbody>${body}</tbody>`;
    return { html: `${html}</table></div>`, end: index - 1 };
  };
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const line = lines[lineIndex];
    if (line.startsWith("```")) {
      inCode = !inCode;
      html += inCode ? `<pre class="${CODE_BLOCK_CLASS}"><code>` : "</code></pre>";
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
    // A row of pipes is a table only when the line under it is a delimiter row.
    // Without that, it is a paragraph that happens to contain pipes — which is
    // what most prose with a pipe in it is.
    const alignments = tableCells(line) ? delimiterAlignments(lines[lineIndex + 1]) : null;
    if (alignments) {
      const table = tableFrom(lineIndex, alignments);
      html += table.html;
      lineIndex = table.end;
      continue;
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
