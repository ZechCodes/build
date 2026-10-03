// The block half of the one markdown renderer: paragraphs, headings, lists,
// quotes, rules, tables and fences, over lines. core/markdown.js `markdownHtml` is the only
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

const HEADING = /^(#{1,6}) (.*)$/;

/** A heading, with an id taken from its raw text so the stages view can
 *  scroll a comment's breadcrumb to `#<slug>`. */
function readHeading(lines, at, context) {
  const [, marks, raw] = HEADING.exec(lines[at]);
  const level = marks.length;
  return { html: `<h${level}${context.idAttr(raw)}>${context.inline(raw)}</h${level}>`, next: at + 1 };
}

/// How deep quotes and lists may nest (#229). Past it a line is read as the
/// words it is, so no input can make the reader recurse without bound.
const MAX_DEPTH = 8;

const QUOTE = /^ {0,3}>/;
const QUOTE_MARK = /^ {0,3}> ?/;

/** A quote: every line in a row that carries the mark — `>` alone is a blank
 *  line inside it — read again as a document of its own, so a quote holds
 *  paragraphs, lists, tables and other quotes. */
function readQuote(lines, at, context) {
  const inner = [];
  const offsets = [];
  let next = at;
  for (; next < lines.length && QUOTE.test(lines[next]); next += 1) {
    const removed = QUOTE_MARK.exec(lines[next])[0].length;
    inner.push(lines[next].slice(removed));
    offsets.push(context.offsets[next] + removed);
  }
  return { html: `<blockquote class="${QUOTE_CLASS}">${blocksOf(inner, { ...deeper(context), offsets })}</blockquote>`, next };
}

/// The class a quote wears, stamped by the renderer for the reason
/// CODE_BLOCK_CLASS is: every surface's quotes look alike.
const QUOTE_CLASS = "md-quote";

/// A thematic break (#253): three or more of one of `-`, `*` or `_`, spaces
/// and tabs allowed between and around them, up to three spaces of indent.
/// A `---` directly under a line of prose is that line's setext underline and
/// the paragraph reader takes it first (SETEXT_UNDERLINE); after a blank line
/// it is a rule. It is tested before a list item, since
/// `- - -` and `* * *` read as both and CommonMark makes them breaks.
const THEMATIC_BREAK = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;

/// The class a rule wears, stamped for the reason CODE_BLOCK_CLASS is.
const RULE_CLASS = "md-rule";

const readRule = (lines, at) => ({ html: `<hr class="${RULE_CLASS}">`, next: at + 1 });

// Nine digits at most, as CommonMark reads a number: longer is prose.
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])( +)(.*)$/;

/** One item line, read: how far its marker is indented, whether it numbers,
 *  where its content starts, and the content. */
function itemAt(line) {
  const match = LIST_ITEM.exec(line ?? "");
  if (!match) return null;
  const [, indent, marker, gap, content] = match;
  const ordered = /\d/.test(marker);
  return { indent: indent.length, ordered, start: ordered ? Number.parseInt(marker, 10) : null, contentAt: indent.length + marker.length + gap.length, content };
}

const indentOf = (line) => line.length - line.trimStart().length;

/** Whether an item of this list starts at the line: the same indent, the same
 *  kind of marker. */
const sameList = (line, first) => {
  const item = itemAt(line);
  return Boolean(item && !THEMATIC_BREAK.test(line) && item.indent === first.indent && item.ordered === first.ordered);
};

/** The line where an item's own lines end: every line indented past its
 *  marker, and blank lines followed by one. */
function itemEnd(lines, from, first) {
  let end = from;
  for (let next = from; next < lines.length; next += 1) {
    const line = lines[next];
    if (line.trim() && indentOf(line) <= first.indent) break;
    if (line.trim()) end = next + 1;
  }
  return end;
}

/** One item: its first line's content and the lines under it, taken back to
 *  the item's own margin. A single line is inline, unless it is a rule
 *  (`* ---`); more is a document, whose first paragraph sits on the item's
 *  line the way a tight list reads. */
function itemHtml(item, body, context) {
  const task = taskItem(item.content, context.offsets[0] + item.contentAt, context);
  const content = task ? item.content.slice(task.length) : item.content;
  const opening = task ? `<li class="task-item">${task.html}` : "<li>";
  if (!body.length && !THEMATIC_BREAK.test(content)) return `${opening}${taskContentsHtml(context.inline(content), task)}</li>`;
  const margins = body.map((line) => Math.min(indentOf(line), item.contentAt));
  const lines = [content, ...body.map((line, index) => line.slice(margins[index]))];
  const offsets = [context.offsets[0] + item.contentAt + (task?.length || 0), ...margins.map((margin, index) => context.offsets[index + 1] + margin)];
  return `${opening}${taskContentsHtml(blocksOf(lines, { ...deeper(context), offsets }), task, true)}</li>`;
}

/** A scoped checkbox names its first rendered paragraph, keeping child tasks
 * and later details outside its accessible name. Block labels remain valid
 * HTML; an empty label gets hidden text instead of naming its child list. */
function taskContentsHtml(html, task, block = false) {
  const paragraph = /^<p>([\s\S]*?)<\/p>/.exec(html);
  const flat = paragraph ? paragraph[1] + html.slice(paragraph[0].length) : html;
  if (!task?.labelId) return flat;
  if (paragraph) return taskLabelSpan(task, paragraph[1]) + html.slice(paragraph[0].length);
  if (!block) return taskLabelSpan(task, task.empty ? task.label : html, task.empty);
  return blockTaskLabelHtml(html, task);
}

const taskLabelSpan = (task, html, hidden = false) =>
  `<span class="md-task-label${hidden ? " sr-only" : ""}" id="${task.labelId}">${html}</span>`;

/** Keep a heading's actual rendered words as the label. Other blocks use the
 * item's own rendered inline name rather than enclosing subsequent children. */
function blockTaskLabelHtml(html, task) {
  const heading = /^(<h[1-6](?: [^>]*)?>)([\s\S]*?)(<\/h[1-6]>)/.exec(html);
  if (heading) return heading[1] + taskLabelSpan(task, heading[2]) + heading[3] + html.slice(heading[0].length);
  return taskLabelSpan(task, task.label, true) + html;
}

/** A checklist mark belongs only at the beginning of a list item's content.
 * Every attribute is fixed, numeric, or escaped. The input stays native for
 * keyboard and accessibility support. A caller's stable document scope keeps
 * label ids separate across bodies and comments while leaving paints stable. */
function taskItem(content, start, context) {
  const match = /^\[([ xX])\](?:[ \t]+|$)/.exec(content);
  if (!match) return null;
  const index = context.taskMarkers.length;
  const offset = start + 1;
  context.taskMarkers.push(offset);
  const raw = content.slice(match[0].length).trim();
  const fallback = `Checklist item ${index + 1}`;
  const labelId = context.taskLabelPrefix ? `${context.taskLabelPrefix}:${index}` : null;
  // Inline output is already escaped; removing its fixed tags retains the
  // escaped attribute text, including a reference's visible resolved label.
  const renderedText = context.inline(raw).replace(/<[^>]*>/g, "").trim();
  const label = renderedText || fallback;
  const name = labelId ? `aria-labelledby="${labelId}"` : `aria-label="${label}"`;
  const checked = match[1] === " " ? "" : " checked";
  const disabled = context.taskItems ? "" : " disabled";
  return { length: match[0].length, labelId, empty: !renderedText, label,
    html: `<input class="md-task-checkbox" type="checkbox" data-task-index="${index}" data-task-offset="${offset}" ${name}${checked}${disabled}>` };
}

/** Code-point encoding is deterministic and injective. Unlike sanitizing or
 * hashing caller scopes, distinct names cannot produce the same safe id. */
const taskLabelPrefix = (scope) => scope == null ? null
  : `md-task-label:${Array.from(String(scope), (character) => character.codePointAt(0).toString(16)).join("-")}`;

/** A list: its items in a row, a blank line between two of them keeping them
 *  one list. It ends at a line that is neither an item of it nor indented
 *  under one. The next filled line is found once per item, so a run of blank
 *  lines costs its length and no more (#238). An ordered list starts at its
 *  first number. */
function readList(lines, at, context) {
  const first = itemAt(lines[at]);
  let items = "";
  let next = at;
  while (next < lines.length && sameList(lines[next], first)) {
    const item = itemAt(lines[next]);
    const end = itemEnd(lines, next + 1, first);
    items += itemHtml(item, lines.slice(next + 1, end), { ...context, offsets: context.offsets.slice(next, end) });
    const filled = nextFilled(lines, end);
    next = sameList(lines[filled], first) ? filled : end;
  }
  if (!first.ordered) return { html: `<ul>${items}</ul>`, next };
  return { html: `<ol${first.start === 1 ? "" : ` start="${first.start}"`}>${items}</ol>`, next };
}

/** The first line at or after `at` that is not blank. */
function nextFilled(lines, at) {
  let next = at;
  while (next < lines.length && !lines[next].trim()) next += 1;
  return next;
}

/** The same reading one level down, or null past the deepest level. */
const deeper = (context) => ({ ...context, depth: context.depth + 1 });

/** Whether a nesting block may open at this depth. */
const nests = (context) => context.depth < MAX_DEPTH;

// ─── Paragraphs ──────────────────────────────────────────────────────────────

/** A line of a paragraph as it joins the one before: a soft break is a space,
 *  and a line ending in two spaces or a backslash breaks with a `<br>`. */
const HARD_BREAK = /( {2,}|\\)$/;

/// A setext underline (#253): `===` under a paragraph makes it a level-1
/// heading and `---` a level-2 one. Unlike a rule it takes no inner spaces, so
/// `- - -` under prose is still a rule.
const SETEXT_UNDERLINE = /^ {0,3}(=+|-+)[ \t]*$/;
const SETEXT_LEVELS = { "=": 1, "-": 2 };

/** The line a paragraph starting here stops at: a blank line, an underline,
 *  or a line another block starts. Its first line is always its own. */
function paragraphEnd(lines, at, context) {
  let next = at + 1;
  const continues = (line) => line.trim() && !SETEXT_UNDERLINE.test(line) && !interrupts(lines, next, context);
  while (next < lines.length && continues(lines[next])) next += 1;
  return next;
}

/** A paragraph's lines as a heading of the level its underline names, the
 *  text joined as one line and its id taken from it, as an ATX heading's is. */
function setextHeading(prose, underline, context) {
  const level = SETEXT_LEVELS[underline.trim()[0]];
  const raw = prose.map((line) => line.trim().replace(/\\$/, "")).join(" ");
  return `<h${level}${context.idAttr(raw)}>${context.inline(raw)}</h${level}>`;
}

/** A paragraph's lines joined the way CommonMark reads prose wrapped at a
 *  width. */
function paragraphHtml(prose, context) {
  const parts = prose.map((line) => {
    const text = context.inline(line.trim().replace(/\\$/, ""));
    return HARD_BREAK.test(line) ? `${text}<br>` : text;
  });
  return `<p>${parts.join(" ").replace(/<br> /g, "<br>")}</p>`;
}

/** The paragraph at this line: every line to a blank one or to a line another
 *  block starts — or, when an underline ends it, the setext heading it is. */
function readParagraph(lines, at, context) {
  const end = paragraphEnd(lines, at, context);
  const prose = lines.slice(at, end);
  const underline = lines[end];
  if (underline !== undefined && SETEXT_UNDERLINE.test(underline)) return { html: setextHeading(prose, underline, context), next: end + 1 };
  return { html: paragraphHtml(prose, context), next: end };
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
  { starts: (lines, at, context) => nests(context) && QUOTE.test(lines[at]), read: readQuote },
  { starts: (lines, at) => THEMATIC_BREAK.test(lines[at]), read: readRule },
  { starts: (lines, at, context) => nests(context) && LIST_ITEM.test(lines[at]), read: readList },
  { starts: tableStarts, read: readTable },
  { starts: (lines, at) => HEADING.test(lines[at]), read: readHeading },
];

const PARAGRAPH = { read: readParagraph };

function interrupts(lines, at, context) {
  return BLOCKS.some((block) => block.starts(lines, at, context));
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
    const block = BLOCKS.find((candidate) => candidate.starts(lines, at, context)) || PARAGRAPH;
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
export function blocksHtml(markdown, inline, { taskItems = false, taskMarkers, taskLabelScope } = {}) {
  const source = String(markdown || "");
  const lines = source.split(/\r?\n/);
  let offset = 0;
  const offsets = lines.map((line) => {
    const start = offset;
    offset += line.length;
    offset += source[offset] === "\r" ? 2 : 1;
    return start;
  });
  return blocksOf(lines, { inline, idAttr: headingIds(), depth: 0, offsets, taskItems, taskMarkers: taskMarkers || [], taskLabelPrefix: taskLabelPrefix(taskLabelScope) });
}
