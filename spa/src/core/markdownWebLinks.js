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

/** Images share the web scheme policy, narrowed to real http(s) URLs
 * with a host and no URL credentials. */
export function safeImageSrc(raw) {
  const href = safeWebHref(raw);
  if (!href || !/^https?:/i.test(href)) return null;
  try {
    const parsed = new URL(href);
    return parsed.hostname && !parsed.username && !parsed.password && !/^https?:\/\/[^/\\]*@/i.test(href) ? href : null;
  } catch {
    return null;
  }
}

function image(src, label, title, imageOrigin) {
  const alt = unesc(label.replace(/<\/?(?:code|kbd)>/g, ""));
  const titleAttr = title ? ` title="${esc(title)}"` : "";
  const url = new URL(src);
  if (url.origin === imageOrigin) {
    return { markup: `<img src="${esc(src)}" alt="${esc(alt)}" loading="lazy" referrerpolicy="no-referrer"${titleAttr}>` };
  }
  // Production CSP admits remote HTTPS images. HTTP keeps its ordinary link
  // affordance after image-specific credential validation has already passed.
  if (url.protocol === "http:") {
    const linkLabel = esc(alt) || esc(src);
    return { markup: anchor(src, linkLabel, title), linkLabel };
  }
  const titleData = title ? ` data-md-image-title="${esc(title)}"` : "";
  const words = `Load image from ${new URL(src).hostname}${alt ? ` — ${alt}` : ""}`;
  return { markup: `<button type="button" class="md-image-load" data-md-image-src="${esc(src)}" data-md-image-alt="${esc(alt)}"${titleData}>${esc(words)}</button>` };
}

/** One web link, from its already-escaped label. */
function anchor(href, label, title) {
  const titleAttr = title ? ` title="${esc(title)}"` : "";
  return `<a class="${WEB_LINK_CLASS}" href="${esc(href)}" target="_blank" rel="noopener noreferrer nofollow"${titleAttr}>${label || esc(href)}</a>`;
}

// ─── Inline links ────────────────────────────────────────────────────────────

const CODE_OPEN = "<code>";
const CODE_CLOSE = "</code>";
const FOOTNOTE_LABEL = /\[\^[A-Za-z0-9_-]+\]/y;

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

/** The explicit `[![alt](image)](link)` production reuses both destination
 * readers. Consuming its outer end prevents already copied text or targets
 * from being scanned again. Other nested bracket syntax stays unchanged. */
function enclosingImageLink(html, span, copied) {
  const start = span.start - 1;
  if (start < copied || html[start] !== "[" || !html.startsWith("](", span.end)) return span;
  const destination = readDestination(html, span.end + 2);
  const href = destination && safeWebHref(unesc(destination.url));
  return href ? { start, end: destination.end, markup: anchor(href, span.linkLabel ?? span.markup, unesc(destination.title)) } : span;
}

/** The link whose label closes at `close`, written to `state` — or nothing,
 *  leaving it as written, when its address is refused. */
function writeLink(html, close, destination, state) {
  const isImage = html[state.opener - 1] === "!";
  const href = isImage ? safeImageSrc(unesc(destination.url)) : safeWebHref(unesc(destination.url));
  const start = state.opener - Number(isImage);
  const label = html.slice(state.opener + 1, close);
  const title = unesc(destination.title);
  const content = href ? (isImage ? image(href, label, title, state.imageOrigin)
    : { markup: anchor(href, state.formatLabel(label), title) }) : { markup: html.slice(start, destination.end) };
  const imageSpan = { start, end: destination.end, ...content };
  const span = isImage ? enclosingImageLink(html, imageSpan, state.copied) : imageSpan;
  state.out += html.slice(state.copied, span.start) + span.markup;
  state.copied = span.end;
  return span.end;
}

/** Literal inline spans and footnote notation inside a label are atomic. */
function pastLiteral(html, index, opener) {
  if (html.startsWith(CODE_OPEN, index)) return pastCode(html, index);
  if (html.startsWith("<kbd>", index)) return html.indexOf("</kbd>", index) + 6;
  FOOTNOTE_LABEL.lastIndex = index;
  const footnoteLabel = opener >= 0 ? FOOTNOTE_LABEL.exec(html) : null;
  return footnoteLabel ? index + footnoteLabel[0].length : null;
}

/** One step of the reader at `index`, answering where the next one starts.
 * The innermost `[` before a `](` opens the label. */
function readAt(html, index, state) {
  const literalEnd = pastLiteral(html, index, state.opener);
  if (literalEnd !== null) return literalEnd;
  if (html[index] === "[") state.opener = index;
  if (html[index] !== "]") return index + 1;
  const destination = state.opener >= 0 && html[index + 1] === "(" ? readDestination(html, index + 2) : null;
  const next = destination ? writeLink(html, index, destination, state) : index + 1;
  state.opener = -1;
  return next;
}

/** Every `[label](address)` in one line of escaped HTML made a link, and
 *  every refused one left as written. */
function inlineLinks(html, formatLabel, imageOrigin) {
  const state = { out: "", copied: 0, opener: -1, formatLabel, imageOrigin };
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
const LITERAL = /<code>[\s\S]*?<\/code>|<kbd>[^<]*<\/kbd>|<a\b[^>]*>[\s\S]*?<\/a>|<img\b[^>]*>|<button\b[^>]*>[\s\S]*?<\/button>|!?\[[^[\]]*\]\([^()[\]]*(?:\([^()[\]]*\)[^()[\]]*)*\)/g;

/** Every bare address in one stretch of prose made a link. */
const bareLinksIn = (text) =>
  text.replace(BARE, (match) => {
    const url = withoutTrailing(match);
    const href = safeWebHref(unesc(url));
    return href ? anchor(href, url) + match.slice(url.length) : match;
  });

/** Every bare address outside code and links made a link. */
function outsideLiterals(html, transform) {
  let out = "";
  let at = 0;
  LITERAL.lastIndex = 0;
  for (let match = LITERAL.exec(html); match; match = LITERAL.exec(html)) {
    out += transform(html.slice(at, match.index)) + match[0];
    at = match.index + match[0].length;
  }
  return out + transform(html.slice(at));
}

const bareLinks = (html) => outsideLiterals(html, bareLinksIn);

/** Angle autolinks are another explicit grammar, never raw tags. */
function angleLinks(html) {
  return html.replace(/&lt;((?:https?:\/\/|mailto:)(?:[^\s<&]|&(?!lt;|gt;))*|(?:[^\s<>&@]|&(?!lt;|gt;))+@(?:[^\s<>&@]|&(?!lt;|gt;))+)&gt;/gi, (raw, address) => {
    const words = unesc(address);
    const href = safeWebHref(words.includes(":") ? words : `mailto:${words}`);
    return href ? anchor(href, address) : raw;
  });
}

/**
 * Web links in one line of escaped HTML: inline links first, so a bare address
 * that is an inline link's label or address is not linked again.
 */
export function webLinks(html, formatLabel = (label) => label, imageOrigin) {
  return html ? bareLinks(outsideLiterals(inlineLinks(html, formatLabel, imageOrigin), angleLinks)) : html;
}
