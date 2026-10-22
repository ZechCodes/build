// A deliberately tiny grammar, never an HTML parser: standalone attribute-free
// details lines, one immediate single-line summary, and a balanced closer.
// Delimiters inside fenced source are ordinary source. Matching in one pass
// prevents repeated unclosed openers from causing quadratic searches.
import { fenceOpening, fenceCloses } from "./markdownFences.js";
import { listBodyEnd } from "./markdownBlockScopes.js";

const OPEN = /^ {0,3}<details>[ \t]*$/;
const CLOSE = /^ {0,3}<\/details>[ \t]*$/;
const SUMMARY = /^ {0,3}<summary>(.*?)<\/summary>[ \t]*$/;

export function detailsPairs(lines) {
  const pairs = new Map();
  const stack = [];
  let fence = null;
  for (let at = 0; at < lines.length; at += 1) {
    if (fence) {
      if (fenceCloses(lines[at], fence)) fence = null;
      continue;
    }
    fence = fenceOpening(lines[at]);
    if (fence) continue;
    const listEnd = listBodyEnd(lines, at);
    if (listEnd > at) { at = listEnd; continue; }
    if (OPEN.test(lines[at])) stack.push(at);
    else if (CLOSE.test(lines[at]) && stack.length) pairs.set(stack.pop(), at);
  }
  return pairs;
}

export const detailsStarts = (lines, at, context) => context.depth < 8
  && context.detailsPairs.has(at) && SUMMARY.test(lines[at + 1] ?? "");

export function readDetails(lines, at, context, render) {
  const end = context.detailsPairs.get(at);
  const summary = SUMMARY.exec(lines[at + 1])[1];
  const label = context.inline(summary);
  const inner = render(lines.slice(at + 2, end), {
    ...context, depth: context.depth + 1, offsets: context.offsets.slice(at + 2, end),
  });
  return { html: `<details class="md-details"><summary>${label}</summary>${inner}</details>`, next: end + 1 };
}
