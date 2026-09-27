// One line of plain text out of a markdown body, for the rows that preview a
// conversation (#186): no table pipes, no heading marks, no emphasis stars,
// no link brackets, and every run of whitespace folded to one space. The
// overview's rows are one line each, and a line that opens with
// "| Issue | Verdict |" says nothing about what the agent said.
//
// This is a reduction, not a parser: it takes the marks off and keeps the
// words, in order. Fenced code is dropped whole, because a line of code is not
// a summary of anything. No DOM.
//
// Every pattern stops at the next mark of its own kind (a tag at the next `<`,
// a link at the next `[` or `)`), so an unclosed mark costs its own fragment
// and not the rest of the body; and only the head of the body is scanned at
// all, since the preview is a line. A 200 KB body of "<a " fragments is the
// shape that once took seconds.

/** How much of a body is worth scanning for a preview of `limit` characters.
 *  Marks come off, so more than the limit; a fence up front can still leave
 *  a short preview, which is the trade for a bounded cost. */
const SCAN_CHARS = 4096;

// A fence of backticks or of tildes, closed by its own kind or by the end.
const FENCE = /(```|~~~)[\s\S]*?(?:\1|$)/g;
// An autolink, <https://…> or <someone@…>, keeps its address; a tag goes.
const AUTOLINK = /<((?:[a-zA-Z][a-zA-Z0-9+.-]*:|[^\s<>@]+@)[^\s<>]*)>/g;
const TAG = /<\/?[a-zA-Z][^<>]*>/g;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(?:\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const HEADING = /^\s{0,3}#{1,6}\s+/;
const QUOTE = /^\s*(?:>\s?)+/;
const LIST_MARK = /^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/;
const RULE = /^\s*(?:[-*_]\s*){3,}$/;
// A link's address may hold one pair of parentheses, as an encyclopedia's do.
const IMAGE = /!\[([^[\]]*)\]\([^()]*(?:\([^()]*\)[^()]*)*\)/g;
const LINK = /\[([^[\]]+)\]\([^()]*(?:\([^()]*\)[^()]*)*\)/g;
const CODE = /`+([^`]*)`+/g;
const STRONG = /(\*\*|__)([^*_](?:.*?[^*_])?)\1/g;
const EMPHASIS = /(^|[\s(])[*_]([^*_\s](?:[^*_]*?[^*_\s])?)[*_](?=[\s).,;:!?]|$)/g;
const STRIKE = /~~([^~]+)~~/g;

/** The plain words of one markdown line: its marks taken off, or "" for a
 *  line that was only marks (a table's rule, a horizontal rule). */
function plainLine(line) {
  if (TABLE_RULE.test(line) || RULE.test(line)) return "";
  return line.replace(HEADING, "").replace(QUOTE, "").replace(LIST_MARK, "");
}

/** Whether a UTF-16 unit is the first half of a surrogate pair. */
const leadsPair = (unit) => unit >= 0xD800 && unit <= 0xDBFF;

/** The first `count` units of `text`, backed off by one where the cut would
 *  split a surrogate pair and leave half an emoji. */
function clip(text, count) {
  if (text.length <= count) return text;
  const kept = leadsPair(text.charCodeAt(count - 1)) ? count - 1 : count;
  return text.slice(0, kept);
}

/** The plain text of a markdown body, on one line, at most `limit` characters
 *  with an ellipsis where it was cut. "" for nothing. */
export function plainPreview(markdown, limit = 240) {
  const text = clip(String(markdown || ""), Math.max(SCAN_CHARS, limit)).replace(FENCE, " ").replace(AUTOLINK, "$1").replace(TAG, " ")
    .split("\n").map(plainLine).join(" ")
    .replace(IMAGE, "$1").replace(LINK, "$1").replace(CODE, "$1")
    .replace(STRONG, "$2").replace(EMPHASIS, "$1$2").replace(STRIKE, "$1")
    .replace(/\|/g, " ")
    .replace(/\s+/g, " ").trim();
  if (text.length <= limit) return text;
  return `${clip(text, Math.max(0, limit - 1)).trimEnd()}…`;
}
