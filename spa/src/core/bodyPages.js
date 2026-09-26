// A body too large for one cache record, kept as pages (#95).
//
// A file or a diff over its cache cap is not refused the cache any more: it
// is read a page at a time — `range` on `fs.read`, `git.diff`, `git.show`,
// `git.changeset_diff` from a bridge announcing `bodies.pages` — and each
// page is a record of its own, beside the body's usual record (its "head"),
// which then holds what the body is without holding its text. A view paints
// the pages the cache holds and asks for the next one as the reader reaches
// the end of them.
//
// A page record is `{ of, offset, end, total, body }`: `of` names the body
// the page was cut from (the bridge's `range.version` — a file's version, a
// digest of the whole patch — or "whole" for an answer split here), so pages
// of two versions of one body are never joined; `offset`/`end` are byte
// offsets into the whole, and `body` is the page's text — patch text, or
// base64 for a file's bytes. The pages of one body chain: each starts at the
// `end` of the one before, from 0.
//
// A bridge that cannot page answered the body whole, or cut at its wire cap.
// That answer is still kept, split into pages here, so the view paints it
// from the cache all the same; what it cannot do is read past a cut.
//
// Retention: pages live and die with their head, under the head's own rule —
// a file's pages go with it when `trimRecentFiles` keeps only the five most
// recently opened, and every page goes with its workspace's data. There is no
// size limit on one body's pages: a reader who scrolls a 100 MB log to the
// end keeps 100 MB until one of those rules drops it (Zech's call on #95,
// 2026-09-26). If a limit is ever wanted, its shape is: once a body passes
// 16 MB, drop the pages furthest from where the reader is.

import { cachedSubKeys, deleteCached, readCachedMany, subscribeCache, writeCached } from "./localCache.js";

/** The most one page asks for: a quarter of a megabyte, the same as the
 *  largest commit patch a single record holds. */
export const BODY_PAGE_BYTES = 262144;

/** The kind every page record is kept under, beside its head's own kind. */
export const PAGE_RECORD_KIND = "page";

/** The sub-key prefix a head's pages share: its kind and sub, so the pages of
 *  a `filediff` of `a.js` never meet those of a `file` of the same path. */
const pagePrefix = (head) => `${head.kind}:${head.sub ?? ""}@`;

const pageAddress = (head, offset) => ({
  deviceId: head.deviceId,
  entityId: head.entityId,
  kind: PAGE_RECORD_KIND,
  sub: `${pagePrefix(head)}${offset}`,
});

const pageKinds = (head) => ({ deviceId: head.deviceId, entityId: head.entityId, kind: PAGE_RECORD_KIND });

/** The offsets of every page held under `head`, whichever body they are of. */
async function heldOffsets(head) {
  const prefix = pagePrefix(head);
  const subs = await cachedSubKeys(head.deviceId, head.entityId, PAGE_RECORD_KIND);
  return (subs || [])
    .filter((sub) => sub.startsWith(prefix))
    .map((sub) => Number(sub.slice(prefix.length)))
    .filter(Number.isFinite);
}

const reachesItsEnd = (page) => page.total !== null && page.total !== undefined && page.end >= page.total;

const NO_PAGES = Object.freeze({ pages: [], end: 0, total: null, complete: false });

/** Whether a page carries the chain on from `end`: it starts there and moves
 *  it forward, or it is the empty page that says `end` is the whole. */
const carriesOn = (page, end) => Boolean(page) && (page.end > end || (page.end === end && reachesItsEnd(page)));

/** The chain of pages from `from`'s end on, one `byOffset` lookup at a time. */
function chainOn(from, byOffset) {
  const pages = [...from.pages];
  let end = from.end;
  for (let page = byOffset.get(end); carriesOn(page, end); page = byOffset.get(end)) {
    pages.push(page);
    end = page.end;
    if (reachesItsEnd(page)) break; // a stale longer body's pages may follow on
  }
  const total = pages.length ? pages[pages.length - 1].total ?? null : null;
  return { pages, end, total, complete: total !== null && pages.length > 0 && end >= total };
}

/** Whether `held` can be read on from: every page it holds is still stored. */
const stillStored = (held, offsets) => held.pages.length > 0 && held.pages.every((page) => offsets.has(page.offset));

/** Every page of `of` held under `head`, in order from 0, stopping at the
 *  first gap: `{ pages, end, total, complete }`. `total` is what the last
 *  page said the whole weighs — null while nothing has said — and `complete`
 *  is whether the pages reach it.
 *
 *  `held`, what an earlier call answered for the same `of`, is read on from
 *  rather than again: a page of one version never changes, so only the pages
 *  past its end are read out of the store — unless one of its pages has gone,
 *  when the whole chain is read afresh. A reader scrolling a long body so
 *  costs each page once, not every page at every step. */
export async function readBodyPages(head, of, held = null) {
  const offsets = new Set(await heldOffsets(head));
  const from = held && stillStored(held, offsets) ? held : NO_PAGES;
  const wanted = [...offsets].filter((offset) => offset >= from.end);
  const records = await readCachedMany(wanted.map((offset) => pageAddress(head, offset)));
  const byOffset = new Map();
  records.forEach((record) => {
    const page = record?.value;
    if (page && page.of === of) byOffset.set(page.offset, page);
  });
  return chainOn(from, byOffset);
}

/** Keep one page. The first page of a body is where a new version starts, so
 *  writing it lets go of every page of any other version first. */
export async function writeBodyPage(head, page) {
  if (page.offset === 0) await dropBodyPages(head, { keep: page.of });
  await writeCached(pageAddress(head, page.offset), {
    of: page.of,
    offset: page.offset,
    end: page.end,
    total: page.total,
    body: page.body,
  });
}

/** Keep every page of one body at once, in order: what a body read whole is
 *  split into. */
export async function writeBodyPages(head, pages) {
  for (const page of pages) await writeBodyPage(head, page);
}

/** Let go of the pages held under `head`: all of them, or all but those of
 *  `keep`. Answers the addresses dropped. */
export async function dropBodyPages(head, { keep = undefined } = {}) {
  const offsets = await heldOffsets(head);
  if (!offsets.length) return [];
  const addresses = offsets.map((offset) => pageAddress(head, offset));
  let doomed = addresses;
  if (keep !== undefined) {
    const records = await readCachedMany(addresses);
    doomed = addresses.filter((_, index) => records[index]?.value?.of !== keep);
  }
  if (doomed.length) await deleteCached(doomed);
  return doomed;
}

/** Hear every write of a page of `head`'s body, from this tab or another —
 *  and every eviction that covers them, which names no page at all. */
export function subscribeBodyPages(head, listener) {
  const prefix = pagePrefix(head);
  return subscribeCache(pageKinds(head), (address) => {
    if (address?.sub === undefined || String(address.sub).startsWith(prefix)) listener(address);
  });
}

/** Where a page cut inside `text` from `start` ends: after its last line end
 *  within `bytes` of UTF-16 units, or at the window's end for one line longer
 *  than a page, or at the text's end. Patch text is measured in characters
 *  here, which is never more than its bytes. */
function textPageEnd(text, start, bytes) {
  const limit = Math.min(text.length, start + bytes);
  if (limit >= text.length) return text.length;
  const newline = text.lastIndexOf("\n", limit - 1);
  if (newline >= start) return newline + 1;
  return splitsAPair(text, limit) ? limit - 1 : limit;
}

/** Whether cutting `text` at `at` would part a surrogate pair — one character
 *  of four UTF-8 bytes, which each half would count as three. */
const splitsAPair = (text, at) => {
  const unit = text.charCodeAt(at - 1);
  return unit >= 0xd800 && unit <= 0xdbff;
};

const utf8Length = (text) => new TextEncoder().encode(text).length;

/**
 * A body of text read whole, as pages: each ends a line, as a bridge's would.
 * A `cut` answer — one the bridge shortened — keeps the whole lines before
 * its cut and no more, and weighs what `total` says, or nothing known (null)
 * when the answer did not say. Offsets are the text's UTF-8 bytes, so a page
 * the bridge answers next, from the last `end`, follows on.
 */
export function textPagesOf(text, { of, total, cut = false, bytes = BODY_PAGE_BYTES }) {
  const whole = String(text || "");
  const usable = cut ? whole.slice(0, whole.lastIndexOf("\n") + 1) : whole;
  const size = total ?? (cut ? null : utf8Length(whole));
  const pages = [];
  let at = 0;
  let offset = 0;
  do {
    const stop = textPageEnd(usable, at, bytes);
    const body = usable.slice(at, stop);
    const end = offset + utf8Length(body);
    pages.push({ of, offset, end, total: size, body });
    at = stop;
    offset = end;
  } while (at < usable.length);
  return pages;
}

/** Base64 of a byte array, a slice at a time: one `fromCharCode` over
 *  megabytes overflows the argument limit. */
export function base64Of(bytes) {
  const SLICE = 0x8000;
  let binary = "";
  for (let at = 0; at < bytes.length; at += SLICE) {
    binary += String.fromCharCode(...bytes.subarray(at, at + SLICE));
  }
  return btoa(binary);
}

export const bytesOfBase64 = (b64) => Uint8Array.from(atob(b64 || ""), (char) => char.charCodeAt(0));

/** Bytes read whole, as pages of at most `bytes` each, base64 in each record. */
export function bytePagesOf(b64, { of, total, bytes = BODY_PAGE_BYTES }) {
  const whole = bytesOfBase64(b64);
  const size = total ?? whole.length;
  const pages = [];
  let offset = 0;
  do {
    const piece = whole.subarray(offset, Math.min(whole.length, offset + bytes));
    pages.push({ of, offset, end: offset + piece.length, total: size, body: base64Of(piece) });
    offset += piece.length;
  } while (offset < whole.length);
  return pages;
}

/** The text of text pages, joined. */
export const joinedText = (pages) => pages.map((page) => page.body).join("");

/** The bytes of base64 pages, joined, as base64. */
export function joinedBase64(pages) {
  if (pages.length === 1) return pages[0].body;
  const parts = pages.map((page) => bytesOfBase64(page.body));
  const whole = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    whole.set(part, at);
    at += part.length;
  }
  return base64Of(whole);
}

/** A page as a bridge answered it: the text of the page and where it sits.
 *  `field` is where the answer carries the page's text. A page is always of
 *  the version the bridge names — nothing else says which body it was cut
 *  from. */
export function pageFromAnswer(answer, field) {
  const range = answer?.range || {};
  return {
    of: range.version,
    offset: byteCount(range.offset),
    end: byteCount(range.end),
    total: byteCount(range.total),
    body: answer?.[field] ?? "",
  };
}

const byteCount = (value) => Number(value) || 0;

/** Whether `page` is the one asked for at `offset`, of `of`, and moves on
 *  from there. Anything else — another version, a bridge that clamped the
 *  offset, a page that carries nothing — is not joined. */
export const pageFollows = (page, offset, of) =>
  Boolean(page) && page.of === of && page.offset === offset && carriesOn(page, offset);

/**
 * One paged body as a view holds it: the pages the cache has, joined, and the
 * next page read on demand. `readPage(offset)` answers the page a bridge cuts
 * from `offset` (see `pageFromAnswer`), or null where the bridge cannot page
 * — asked at the moment of reading, so a view mounted before its bridge
 * greeted reads on once it has; `onChange(state)` hears every change to what
 * the cache holds; `isCurrent()` says whether the body is still the one the
 * cache holds, checked before a page is kept, so a page that lands after its
 * head was dropped or replaced is never written without one.
 *
 * The view paints `state()` — never an answer — and calls `more()` as the
 * reader reaches the end of what is painted. A page of another version is
 * not joined to these: `onMoved(page)` is told, so the owner can start over
 * from the first page of the new one.
 */
export function createPagedBody({
  head,
  of,
  readPage = null,
  isCurrent = async () => true,
  onChange = () => {},
  onMoved = () => {},
}) {
  let state = NO_PAGES;
  let reading = null;
  let disposed = false;

  const hydrate = async () => {
    const held = await readBodyPages(head, of, state);
    if (disposed) return state;
    state = held;
    onChange(state);
    return state;
  };
  const unwatch = subscribeBodyPages(head, () => void hydrate());

  const keep = async (page, from) => {
    if (!pageFollows(page, from, of) || !(await isCurrent()) || disposed) return false;
    await writeBodyPage(head, page);
    await hydrate();
    return state.end > from;
  };

  const readNext = async () => {
    const from = state.end;
    const page = await readPage(from);
    if (disposed || !page) return false;
    if (page.of !== of) {
      onMoved(page);
      return false;
    }
    return keep(page, from);
  };

  return {
    state: () => state,
    hydrate,
    /** Read the page after the last one held. Answers whether the held pages
     *  moved on; a call while one is being read waits for that one. */
    more() {
      if (reading) return reading;
      if (state.complete || !readPage || !state.pages.length) return Promise.resolve(false);
      reading = readNext()
        .catch(() => false)
        .finally(() => {
          reading = null;
        });
      return reading;
    },
    dispose() {
      disposed = true;
      unwatch();
    },
  };
}
