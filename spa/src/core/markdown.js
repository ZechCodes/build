// Minimal, safe markdown → HTML for plans and notification messages.
// Everything is HTML-escaped; only the small vocabulary below is rendered.

import { esc } from "./text.js";
import { slugifyHeading } from "./anchors.js";
import { expandReferences } from "./markdownLinks.js";

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
 * task page's docs. A bare `pre` rule would reach further than markdown — the
 * terminal, the diff view, the revision pane all use one.
 *
 * So the renderer marks its own output (#50). Wherever it is rendered, the
 * block scrolls; nothing else does.
 */
export const CODE_BLOCK_CLASS = "md-code";

/** The paragraph being read. Consecutive text lines are one paragraph joined by
 *  a space (a soft break), the way CommonMark reads prose wrapped at a line
 *  width; a line ending in two spaces or a backslash breaks it with a `<br>`.
 *  A blank line ends it, and so does any block: `flush` hands back the `<p>`
 *  and empties it. */
function paragraphReader(inline) {
  let parts = [];
  return {
    add(line) {
      if (!line.trim()) return false;
      const hard = /( {2,}|\\)$/.test(line);
      const text = inline(line.trim().replace(/\\$/, ""));
      parts.push(hard ? `${text}<br>` : text);
      return true;
    },
    flush() {
      const html = parts.length ? `<p>${parts.join(" ").replace(/<br> /g, "<br>")}</p>` : "";
      parts = [];
      return html;
    },
  };
}

/**
 * Markdown, and the references an agent can write in it (#56).
 *
 * `links` is optional and resolves a reference to where it lives
 * (core/markdownLinks.js). Without one — which is every caller today — a
 * reference renders as the words that were typed, so adding this took no
 * caller with it and a reference to something missing is never a broken link.
 */
// eslint-disable-next-line complexity -- ratchet: renderMarkdown is at 18, cap 10 — reduce it, then drop this line
export function renderMarkdown(markdown, { links = null } = {}) {
  const lines = (markdown || "").split("\n");
  let html = "";
  let inCode = false;
  let inList = false;
  // References expand AFTER the code spans and never inside one: a message
  // explaining this syntax is mostly examples, and they have to stay literal.
  const inline = (s) =>
    expandReferences(
      esc(s)
        .replace(/`([^`]+)`/g, "<code>$1</code>")
        .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>"),
      links,
    );
  // Every block closes the paragraph before it, so each goes out through here.
  const paragraph = paragraphReader(inline);
  const emit = (block) => {
    html += paragraph.flush() + block;
  };
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
      emit(inCode ? `<pre class="${CODE_BLOCK_CLASS}"><code>` : "</code></pre>");
      continue;
    }
    if (inCode) {
      html += esc(line) + "\n";
      continue;
    }
    const listItem = line.match(/^\s*[-*]\s+(.*)/) || line.match(/^\s*\d+\.\s+(.*)/);
    if (listItem) {
      if (!inList) {
        emit("<ul>");
        inList = true;
      }
      emit(`<li>${inline(listItem[1])}</li>`);
      continue;
    }
    if (inList) {
      emit("</ul>");
      inList = false;
    }
    // A row of pipes is a table only when the line under it is a delimiter row.
    // Without that, it is a paragraph that happens to contain pipes — which is
    // what most prose with a pipe in it is.
    const alignments = tableCells(line) ? delimiterAlignments(lines[lineIndex + 1]) : null;
    if (alignments) {
      const table = tableFrom(lineIndex, alignments);
      emit(table.html);
      lineIndex = table.end;
      continue;
    }
    if (line.startsWith("### ")) {
      const raw = line.slice(4);
      emit(`<h3${idAttr(raw)}>${inline(raw)}</h3>`);
    } else if (line.startsWith("## ")) {
      const raw = line.slice(3);
      emit(`<h2${idAttr(raw)}>${inline(raw)}</h2>`);
    } else if (line.startsWith("# ")) {
      const raw = line.slice(2);
      emit(`<h1${idAttr(raw)}>${inline(raw)}</h1>`);
    } else if (!paragraph.add(line)) emit("");
  }
  emit("");
  if (inList) html += "</ul>";
  if (inCode) html += "</code></pre>";
  return html;
}
