// Web links in what an agent writes (#256): `[text](url)`, `[text](url "title")`
// and a bare `http(s)://…` in prose.
//
// This is the one place the renderer writes an href that is not a Build route,
// so it is the XSS surface. Three rules keep it narrow:
//
//   - Only http:, https: and mailto: are ever a link (`safeWebHref`). The
//     scheme is read after every whitespace and control character is taken
//     out, case-insensitively, so `JaVa\tScRiPt:` is still javascript:. An
//     http(s) address must say `//` and a host; `//host`, `#42` and relative
//     paths are refused, because a Build reference has its own syntax
//     (core/markdownRefs.js) and a relative path has no page to be relative to.
//   - A refused link is the words the agent typed, escaped, not a link.
//   - The href and title are unescaped from the line and escaped again for
//     their attribute; the label is the line's own escaped markup.
//
// Every link opens outside Build: `target="_blank"` with
// `rel="noopener noreferrer nofollow"`. In the desktop app, the window-open
// handler hands http, https and mailto to the system browser
// (desktop/src/main.mjs `setWindowOpenHandler`).
//
// Both readers run over one line of escaped HTML, and a code span in it is
// literal: nothing inside `<code>…</code>` is read, and an address may not
// cross into one. Each is linear: an address stops at the next bracket, so no
// two addresses are read over the same characters.

import { esc, unesc } from "./text.js";

/// The class every web link wears; the stylesheet marks it as leaving Build.
const WEB_LINK_CLASS = "md-link";

/// What each allowed scheme must look like, whole, after cleaning.
const SCHEMES = {
  http: /^http:\/\/[^/\\]/i,
  https: /^https:\/\/[^/\\]/i,
  mailto: /^mailto:[^@]+@[^@]+$/i,
};

/// Whitespace and control characters: C0, space, DEL and C1.
const INVISIBLE = /[\u0000- \u007f-\u009f]/g;

/**
 * The address to write for `raw` as a web link, or null when it may not be
 * one. `raw` is the address as the agent wrote it, unescaped.
 */
export function safeWebHref(raw) {
  const cleaned = String(raw ?? "").replace(INVISIBLE, "");
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(cleaned)?.[1].toLowerCase();
  return Object.hasOwn(SCHEMES, scheme ?? "") && SCHEMES[scheme].test(cleaned) ? cleaned : null;
}

/** One web link, from its already-escaped label. */
function anchor(href, label, title) {
  const titleAttr = title ? ` title="${esc(title)}"` : "";
  return `<a class="${WEB_LINK_CLASS}" href="${esc(href)}" target="_blank" rel="noopener noreferrer nofollow"${titleAttr}>${label || esc(href)}</a>`;
}

// ─── Inline links ────────────────────────────────────────────────────────────

const CODE_OPEN = "<code>";
const CODE_CLOSE = "</code>";

/// The quotes a title may be written in, as they read once escaped.
const TITLE_QUOTES = ["&quot;", "&#39;"];

/// Characters that end any address or title: a bracket starts the next link,
/// and `<` is the renderer's own tag.
const STOPS = new Set(["[", "]", "<"]);

/** An address starting at `at`: characters up to whitespace or the `)` that
 *  balances, parentheses inside kept when they pair. */
function readAddress(html, at) {
  let depth = 0;
  let index = at;
  for (; index < html.length; index += 1) {
    const character = html[index];
    if (STOPS.has(character) || /\s/.test(character)) break;
    if (character === "(") depth += 1;
    if (character === ")" && depth-- === 0) break;
  }
  return index > at && depth <= 0 ? { url: html.slice(at, index), end: index } : null;
}

/** A title starting at `at` in one of TITLE_QUOTES, or null. */
function readTitle(html, at) {
  const quote = TITLE_QUOTES.find((mark) => html.startsWith(mark, at));
  if (!quote) return null;
  const from = at + quote.length;
  for (let index = from; index < html.length && !STOPS.has(html[index]); index += 1) {
    if (html.startsWith(quote, index)) return { title: html.slice(from, index), end: index + quote.length };
  }
  return null;
}

const skipSpaces = (html, at) => {
  let index = at;
  while (html[index] === " ") index += 1;
  return index;
};

/** The `(address "title")` half of a link, opening at `at` (just past the
 *  `(`), or null when it does not close. */
function readDestination(html, at) {
  const address = readAddress(html, at);
  if (!address) return null;
  const titleAt = skipSpaces(html, address.end);
  const title = titleAt > address.end ? readTitle(html, titleAt) : null;
  const close = skipSpaces(html, title ? title.end : address.end);
  if (html[close] !== ")") return null;
  return { url: address.url, title: title?.title ?? "", end: close + 1 };
}

/** Past the code span opening at `at`: it is stepped over whole. */
function pastCode(html, at) {
  const close = html.indexOf(CODE_CLOSE, at);
  return close < 0 ? html.length : close + CODE_CLOSE.length;
}

/** The link whose label closes at `close`, written to `state` — or nothing,
 *  leaving it as written, when its address is refused. */
function writeLink(html, close, destination, state) {
  const href = safeWebHref(unesc(destination.url));
  if (!href) return;
  const label = html.slice(state.opener + 1, close);
  state.out += html.slice(state.copied, state.opener) + anchor(href, label, unesc(destination.title));
  state.copied = destination.end;
}

/** One step of the reader at `index`, answering where the next one starts.
 *  The innermost `[` before a `](` opens the label. */
function readAt(html, index, state) {
  if (html.startsWith(CODE_OPEN, index)) return pastCode(html, index);
  if (html[index] === "[") state.opener = index;
  if (html[index] !== "]") return index + 1;
  const destination = state.opener >= 0 && html[index + 1] === "(" ? readDestination(html, index + 2) : null;
  if (destination) writeLink(html, index, destination, state);
  state.opener = -1;
  return destination ? destination.end : index + 1;
}

/** Every `[label](address)` in one line of escaped HTML made a link, and
 *  every refused one left as written. */
function inlineLinks(html) {
  const state = { out: "", copied: 0, opener: -1 };
  for (let index = 0; index < html.length; ) index = readAt(html, index, state);
  return state.out + html.slice(state.copied);
}

// ─── Bare addresses ──────────────────────────────────────────────────────────

/// A bare address: `http://` or `https://` not glued to a word or a path, then
/// everything to whitespace, a tag, or an escaped quote or angle bracket.
const BARE = /(?<![\w/])https?:\/\/(?:[^\s<&]|&amp;)+/gi;

/// What ends a sentence rather than an address.
const TRAILING = new Set([".", ",", ":", ";", "!", "?", "*", "_", "~", "]"]);

/** A bare address without the punctuation after it: the sentence's marks go,
 *  and a `)` goes when it closes nothing the address opened. */
function withoutTrailing(url) {
  let end = url.length;
  let unclosed = (url.match(/\)/g) || []).length - (url.match(/\(/g) || []).length;
  for (; end > 0; end -= 1) {
    const character = url[end - 1];
    if (character === ")" && unclosed > 0) unclosed -= 1;
    else if (!TRAILING.has(character)) break;
  }
  return url.slice(0, end);
}

/// Stretches a bare address is never read in: a code span, and a link.
const LITERAL = /<code>[\s\S]*?<\/code>|<a\b[^>]*>[\s\S]*?<\/a>/g;

/** Every bare address in one stretch of prose made a link. */
const bareLinksIn = (text) =>
  text.replace(BARE, (match) => {
    const url = withoutTrailing(match);
    const href = safeWebHref(unesc(url));
    return href ? anchor(href, url) + match.slice(url.length) : match;
  });

/** Every bare address outside code and links made a link. */
function bareLinks(html) {
  let out = "";
  let at = 0;
  LITERAL.lastIndex = 0;
  for (let match = LITERAL.exec(html); match; match = LITERAL.exec(html)) {
    out += bareLinksIn(html.slice(at, match.index)) + match[0];
    at = match.index + match[0].length;
  }
  return out + bareLinksIn(html.slice(at));
}

/**
 * Web links in one line of escaped HTML: inline links first, so a bare address
 * that is an inline link's label or address is not linked again.
 */
export function webLinks(html) {
  return html ? bareLinks(inlineLinks(html)) : html;
}
