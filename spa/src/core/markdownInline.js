// Inline markdown stays deliberately small: escaped text, exact backtick
// runs, attribute-free keyboard notation, emphasis, and policy-checked links.
// Nothing here interprets arbitrary HTML. Complete generated elements are
// protected before any subsequent reader sees their text or attributes.
import { esc } from "./text.js";
import { expandReferences } from "./markdownLinks.js";
import { webLinks } from "./markdownWebLinks.js";

// Both readers share the same protected-element grammar. Generated missing
// references are spans whose labels and titles must remain literal too.
const COMPLETE = /<code>[\s\S]*?<\/code>|<kbd>[^<]*<\/kbd>|<a\b[^>]*>[\s\S]*?<\/a>|<span\b[^>]*>[\s\S]*?<\/span>|<sup\b[^>]*>[\s\S]*?<\/sup>|<img\b[^>]*>/g;
const INLINE_TOKEN = new RegExp(`${COMPLETE.source}|&(?:#\\d+|#x[\\da-f]+|[a-z]+);|\\[\\^[A-Za-z0-9_-]+\\]|\\*+|_+|~+`, "gi");
const ESCAPABLE = /\\([!*_~`[\]<>])/g;
const TAGS = { "*": ["em", "strong"], _: ["em", "strong"], "~": [null, "del"] };

/** Every backtick run knows its next equal-length run. Building this once
 * avoids rescanning a long suffix for each unmatched opening delimiter. */
function codeClosers(source) {
  const runs = [...source.matchAll(/`+/g)];
  const next = new Map();
  const closers = new Map();
  for (const run of runs.reverse()) {
    closers.set(run.index, next.get(run[0].length));
    next.set(run[0].length, run.index);
  }
  return closers;
}

/** Escape prose, including escaped punctuation as entities so it cannot
 * become syntax in a later pass. The entity decodes to the original glyph. */
function escapedProse(source) {
  let out = "";
  let at = 0;
  ESCAPABLE.lastIndex = 0;
  for (let match = ESCAPABLE.exec(source); match; match = ESCAPABLE.exec(source)) {
    out += esc(source.slice(at, match.index)) + `&#${match[1].charCodeAt(0)};`;
    at = match.index + match[0].length;
  }
  return out + esc(source.slice(at));
}

/** Keyboard notation is an explicit text-only production, not an HTML
 * allowlist: exact lower-case tags, no attributes and no nested angle marks. */
function literalAt(source, match, closers) {
  if (match[0] === "<kbd>") {
    const end = source.indexOf("<", match.index + 5);
    return source.startsWith("</kbd>", end) ? { end: end + 6, html: `<kbd>${esc(source.slice(match.index + 5, end))}</kbd>` } : null;
  }
  const close = closers.get(match.index);
  return close === undefined ? null : { end: close + match[0].length, html: `<code>${esc(source.slice(match.index + match[0].length, close))}</code>` };
}

/** Exact backtick and keyboard spans are emitted whole before other syntax. */
function literalsHtml(source) {
  const closers = codeClosers(source);
  const marks = /\\.|`+|<kbd>/g;
  let at = 0;
  let out = "";
  for (let match = marks.exec(source); match; match = marks.exec(source)) {
    if (match[0][0] === "\\") continue;
    const literal = literalAt(source, match, closers);
    if (!literal) continue;
    out += escapedProse(source.slice(at, match.index)) + literal.html;
    at = marks.lastIndex = literal.end;
  }
  return out + escapedProse(source.slice(at));
}

/** A matched opener retires intervening unmatched openers. Each delimiter
 * enters and leaves this stack at most once, including malformed input. */
function closeDelimiter(state, opener) {
  while (state.stack.length > opener.position) {
    const removed = state.stack.pop();
    state.latest.set(removed.mark, removed.previous);
  }
  opener.token.html = `<${opener.tag}>`;
  state.tokens.push({ html: `</${opener.tag}>` });
}

function openDelimiters(state, mark, count, canOpen) {
  let remaining = count;
  while (remaining > 0) {
    const width = remaining % 2 || 2;
    const token = { html: mark.repeat(width) };
    const tag = TAGS[mark][width - 1];
    state.tokens.push(token);
    if (canOpen && tag) {
      const opener = { token, tag, mark, width, position: state.stack.length, previous: state.latest.get(mark) };
      state.stack.push(opener);
      state.latest.set(mark, opener);
    }
    remaining -= width;
  }
}

/** Underscores inside words stay words; stars may emphasize within words.
 * Whitespace cannot be the first or last character of an emphasized run. */
function delimiterSides(html, match) {
  const before = html[match.index - 1] || " ";
  const after = html[match.index + match[0].length] || " ";
  const withinWord = match[0][0] === "_" && /[\p{L}\p{N}]/u.test(before) && /[\p{L}\p{N}]/u.test(after);
  return { canOpen: !withinWord && !/\s/.test(after), canClose: !withinWord && !/\s/.test(before) };
}

function emphasisRun(html, match, state) {
  const mark = match[0][0];
  const { canOpen, canClose } = delimiterSides(html, match);
  let remaining = match[0].length;
  let opener = state.latest.get(mark);
  while (canClose && opener && opener.width <= remaining) {
    remaining -= opener.width;
    closeDelimiter(state, opener);
    opener = state.latest.get(mark);
  }
  openDelimiters(state, mark, remaining, canOpen);
}

/** Format only text tokens. Anchors, images, code, keyboard text and escaped
 * entities remain atomic; emphasis can surround them without entering them. */
function emphasisHtml(html) {
  const state = { tokens: [], stack: [], latest: new Map() };
  let at = 0;
  INLINE_TOKEN.lastIndex = 0;
  for (let match = INLINE_TOKEN.exec(html); match; match = INLINE_TOKEN.exec(html)) {
    state.tokens.push({ html: html.slice(at, match.index) });
    if (TAGS[match[0][0]]) emphasisRun(html, match, state);
    else state.tokens.push({ html: match[0] });
    at = match.index + match[0].length;
  }
  state.tokens.push({ html: html.slice(at) });
  return state.tokens.map((token) => token.html).join("");
}

/** Generated elements are closed vocabulary. Their contents and attributes
 * are never handed back to the reference or footnote readers. */
function outsideComplete(html, transform) {
  let out = "";
  let at = 0;
  const complete = new RegExp(COMPLETE.source, "g");
  for (let match = complete.exec(html); match; match = complete.exec(html)) {
    out += transform(html.slice(at, match.index)) + match[0];
    at = match.index + match[0].length;
  }
  return out + transform(html.slice(at));
}

export function inlineHtml(text, links, { footnote } = {}) {
  const web = webLinks(literalsHtml(text), emphasisHtml);
  const notes = footnote ? outsideComplete(web, (run) => run.replace(/\[\^([A-Za-z0-9_-]+)\]/g, (raw, id) => footnote(id) || raw)) : web;
  const references = outsideComplete(notes, (prose) => expandReferences(prose, links));
  return emphasisHtml(references);
}
