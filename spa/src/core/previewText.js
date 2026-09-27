// One line of plain text out of a markdown body, for the rows that preview a
// conversation (#186): no table pipes, no heading marks, no emphasis stars,
// no link brackets, and every run of whitespace folded to one space. The
// overview's rows are one line each, and a line that opens with
// "| Issue | Verdict |" says nothing about what the agent said.
//
// This is a reduction, not a parser: it takes the marks off and keeps the
// words, in order. Fenced code is dropped whole, because a line of code is not
// a summary of anything. No DOM.

const FENCE = /```[\s\S]*?(?:```|$)/g;
const TAG = /<\/?[a-zA-Z][^>]*>/g;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(?:\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const HEADING = /^\s{0,3}#{1,6}\s+/;
const QUOTE = /^\s*(?:>\s?)+/;
const LIST_MARK = /^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/;
const RULE = /^\s*(?:[-*_]\s*){3,}$/;
const IMAGE = /!\[([^\]]*)\]\([^)]*\)/g;
const LINK = /\[([^\]]+)\]\([^)]*\)/g;
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

/** The plain text of a markdown body, on one line, at most `limit` characters
 *  with an ellipsis where it was cut. "" for nothing. */
export function plainPreview(markdown, limit = 240) {
  const text = String(markdown || "").replace(FENCE, " ").replace(TAG, " ")
    .split("\n").map(plainLine).join(" ")
    .replace(IMAGE, "$1").replace(LINK, "$1").replace(CODE, "$1")
    .replace(STRONG, "$2").replace(EMPHASIS, "$1$2").replace(STRIKE, "$1")
    .replace(/\|/g, " ")
    .replace(/\s+/g, " ").trim();
  if (text.length <= limit) return text;
  return `${text.slice(0, Math.max(0, limit - 1)).trimEnd()}…`;
}
