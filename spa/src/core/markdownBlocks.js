// The block half of the one markdown renderer: paragraphs, headings, lists,
// tables and fences, over lines. core/markdown.js `markdownHtml` is the only
// caller (spa/test/markdownEntry.test.js); the inline vocabulary — code spans,
// strong, references — is handed in as `inline`, which escapes everything it is
// given before adding its own fixed tags.
//
// Each block is a reader: whether a line starts one, and how many lines it
// takes and what it draws of them. The document is read by asking each in turn
// at the current line, and a paragraph — which takes whatever no other block
// starts — goes last. No block is a branch of one long function, which is what
// the renderer was until #229 and why it carried a complexity exemption.
//
// Nothing here emits a character of the input without escaping it, and nothing
// emits a URL: the only tags are this file's own.

import { esc } from "./text.js";
import { slugifyHeading } from "./anchors.js";

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
const CODE_BLOCK_CLASS = "md-code";

// ─── Tables ──────────────────────────────────────────────────────────────────

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

/** A delimiter cell's alignment, by which ends carry a colon. */
const ALIGNMENTS = { "::": "center", "-:": "right", ":-": "left", "--": null };
const alignmentOf = (cell) => ALIGNMENTS[`${cell.startsWith(":") ? ":" : "-"}${cell.endsWith(":") ? ":" : "-"}`];

/** The `|---|:--:|` line under a header: every cell is dashes, optionally
 *  colon-anchored. It is what tells a table from a paragraph that happens to
 *  contain pipes, so nothing renders as a table without one. */
function delimiterAlignments(line) {
  const cells = line == null ? null : tableCells(line);
  if (!cells || !cells.length || !cells.every((cell) => /^:?-+:?$/.test(cell))) return null;
  return cells.map(alignmentOf);
}

/** Whether a table starts at this line: a row with a delimiter row under it. */
const tableStarts = (lines, at) => Boolean(tableCells(lines[at]) && delimiterAlignments(lines[at + 1]));

/** A whole table, header row through its last body row.
 *
 *  Every row is fitted to the header's column count — a short row is padded
 *  and a long one truncated — because a table with ragged rows reads as a
 *  rendering bug rather than as the source's own raggedness. The table scrolls
 *  inside .mdtable: the thread panel is a narrow column, and a wide table has
 *  to give way rather than take the conversation's width with it. */
function readTable(lines, at, context) {
  const alignments = delimiterAlignments(lines[at + 1]);
  const columns = tableCells(lines[at]).slice(0, alignments.length);
  const align = (index) => (alignments[index] ? ` style="text-align:${alignments[index]}"` : "");
  const cellsHtml = (cells, tag) =>
    alignments.map((_, index) => `<${tag}${align(index)}>${context.inline(cells[index] ?? "")}</${tag}>`).join("");
  let body = "";
  let next = at + 2; // the header and its delimiter row
  for (let cells = tableCells(lines[next] ?? ""); next < lines.length && cells; cells = tableCells(lines[next] ?? "")) {
    body += `<tr>${cellsHtml(cells, "td")}</tr>`;
    next += 1;
  }
  const head = `<thead><tr>${cellsHtml(columns, "th")}</tr></thead>`;
  return { html: `<div class="mdtable"><table>${head}${body ? `<tbody>${body}</tbody>` : ""}</table></div>`, next };
}

// ─── Fences, headings, lists ─────────────────────────────────────────────────

const FENCE = /^```/;

/** A fenced block: every line to the closing fence — or to the end, when the
 *  document stops inside one — escaped and nothing else. */
function readFence(lines, at) {
  let html = `<pre class="${CODE_BLOCK_CLASS}"><code>`;
  let next = at + 1;
  for (; next < lines.length && !FENCE.test(lines[next]); next += 1) html += `${esc(lines[next])}\n`;
  return { html: `${html}</code></pre>`, next: next + 1 };
}

const HEADING = /^(#{1,3}) (.*)$/;

/** A heading, with an id taken from its raw text so the stages view can
 *  scroll a comment's breadcrumb to `#<slug>`. */
function readHeading(lines, at, context) {
  const [, marks, raw] = HEADING.exec(lines[at]);
  const level = marks.length;
  return { html: `<h${level}${context.idAttr(raw)}>${context.inline(raw)}</h${level}>`, next: at + 1 };
}

const LIST_ITEM = /^\s*(?:[-*]|\d+\.)\s+(.*)/;

/** A list: every item line in a row, each one line of inline markdown. */
function readList(lines, at, context) {
  let items = "";
  let next = at;
  for (let item = LIST_ITEM.exec(lines[next] ?? ""); next < lines.length && item; item = LIST_ITEM.exec(lines[next] ?? "")) {
    items += `<li>${context.inline(item[1])}</li>`;
    next += 1;
  }
  return { html: `<ul>${items}</ul>`, next };
}

// ─── Paragraphs ──────────────────────────────────────────────────────────────

/** A line of a paragraph as it joins the one before: a soft break is a space,
 *  and a line ending in two spaces or a backslash breaks with a `<br>`. */
const HARD_BREAK = /( {2,}|\\)$/;

/** The paragraph at this line: every line to a blank one or to a line another
 *  block starts, joined the way CommonMark reads prose wrapped at a width. */
function readParagraph(lines, at, context) {
  const parts = [];
  let next = at;
  do {
    const line = lines[next];
    const text = context.inline(line.trim().replace(/\\$/, ""));
    parts.push(HARD_BREAK.test(line) ? `${text}<br>` : text);
    next += 1;
  } while (next < lines.length && lines[next].trim() && !interrupts(lines, next));
  return { html: `<p>${parts.join(" ").replace(/<br> /g, "<br>")}</p>`, next };
}

/** A blank line ends whatever came before it and draws nothing. */
const readBlank = (lines, at) => ({ html: "", next: at + 1 });

/**
 * Every block, in the order a line is offered to them. `starts` says whether
 * the block begins at this line; `read` takes it. A paragraph starts anywhere
 * and so goes last, and a line that starts any other block ends a paragraph.
 */
const BLOCKS = [
  { starts: (lines, at) => !lines[at].trim(), read: readBlank },
  { starts: (lines, at) => FENCE.test(lines[at]), read: readFence },
  { starts: (lines, at) => LIST_ITEM.test(lines[at]), read: readList },
  { starts: tableStarts, read: readTable },
  { starts: (lines, at) => HEADING.test(lines[at]), read: readHeading },
];

const PARAGRAPH = { read: readParagraph };

function interrupts(lines, at) {
  return BLOCKS.some((block) => block.starts(lines, at));
}

/** Heading ids, de-duplicated within one document in document order. An empty
 *  slug omits the attribute. */
function headingIds() {
  const counts = new Map();
  return (raw) => {
    const base = slugifyHeading(raw);
    if (!base) return "";
    const count = (counts.get(base) || 0) + 1;
    counts.set(base, count);
    return ` id="${count === 1 ? base : `${base}-${count}`}"`;
  };
}

/** The blocks of a run of lines, each read by the first reader that starts
 *  at its line. */
function blocksOf(lines, context) {
  let html = "";
  for (let at = 0; at < lines.length; ) {
    const block = BLOCKS.find((candidate) => candidate.starts(lines, at)) || PARAGRAPH;
    const read = block.read(lines, at, context);
    html += read.html;
    at = read.next;
  }
  return html;
}

/**
 * Markdown as blocks. `inline` renders one line of the inline vocabulary and
 * escapes everything it is given.
 */
export function blocksHtml(markdown, inline) {
  return blocksOf(String(markdown || "").split("\n"), { inline, idAttr: headingIds() });
}
